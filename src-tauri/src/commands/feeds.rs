//! Feeds (Labs) commands: subscribe, refresh, read, OPML. Storage and parsing
//! are `crate::feeds`; this module owns the network side and the Tauri surface.
//! See docs/design/feeds-labs.md.

use super::err;
use crate::feeds::{self, Feed, FeedItem};
use crate::{db, AppState};
use reqwest::header::{ACCEPT, CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use std::time::Duration;
use futures_util::StreamExt;
use tauri::{AppHandle, Emitter, Manager, State};

/// A feed document is small; anything past this is not one.
const MAX_FEED_BYTES: usize = 20 * 1024 * 1024;

pub(crate) fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("Chatham/", env!("CARGO_PKG_VERSION"), " (+https://github.com/pbayne/chatham)"))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(8))
        .build()
        .map_err(err)
}

fn normalize_url(input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Enter a feed or website address".into());
    }
    let with_scheme = if t.contains("://") { t.to_string() } else { format!("https://{t}") };
    let u = reqwest::Url::parse(&with_scheme).map_err(|_| format!("Not a web address: {t}"))?;
    match u.scheme() {
        "http" | "https" => Ok(u.to_string()),
        s => Err(format!("Unsupported address scheme: {s}")),
    }
}

enum Fetched {
    Body { bytes: Vec<u8>, final_url: String, content_type: String, etag: String, last_modified: String },
    NotModified,
}

async fn fetch(client: &reqwest::Client, url: &str, etag: &str, last_modified: &str) -> Result<Fetched, String> {
    let mut req = client.get(url).header(
        ACCEPT,
        "application/rss+xml, application/atom+xml, application/feed+json, application/xml;q=0.9, text/xml;q=0.9, text/html;q=0.5, */*;q=0.1",
    );
    if !etag.is_empty() {
        req = req.header(IF_NONE_MATCH, etag);
    }
    if !last_modified.is_empty() {
        req = req.header(IF_MODIFIED_SINCE, last_modified);
    }
    let resp = req.send().await.map_err(|e| format!("Couldn't reach {url}: {e}"))?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    if !resp.status().is_success() {
        return Err(format!("{url} answered {}", resp.status()));
    }
    let header = |h| resp.headers().get(h).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let content_type = header(CONTENT_TYPE);
    let etag = header(ETAG);
    let last_modified = header(LAST_MODIFIED);
    let final_url = resp.url().to_string();
    let bytes = resp.bytes().await.map_err(err)?;
    if bytes.len() > MAX_FEED_BYTES {
        return Err("That document is too large to be a feed".into());
    }
    Ok(Fetched::Body { bytes: bytes.to_vec(), final_url, content_type, etag, last_modified })
}

/// Fetch `url`; if it is an HTML page, follow its advertised feed once.
async fn fetch_feed_document(client: &reqwest::Client, url: &str) -> Result<(String, feeds::ParsedFeed, String, String), String> {
    let Fetched::Body { bytes, final_url, content_type, etag, last_modified } = fetch(client, url, "", "").await? else {
        return Err("Unexpected 304 on first fetch".into());
    };
    match feeds::parse(&bytes, &final_url) {
        Ok(p) => Ok((final_url, p, etag, last_modified)),
        Err(parse_err) => {
            let looks_html = content_type.contains("html") || bytes.windows(5).take(2048).any(|w| w.eq_ignore_ascii_case(b"<html"));
            if !looks_html {
                return Err(parse_err.to_string());
            }
            let html = String::from_utf8_lossy(&bytes);
            let feed_url = feeds::discover_feed_link(&html, &final_url)
                .ok_or_else(|| "That page doesn't advertise a feed. Paste the feed's own address.".to_string())?;
            let Fetched::Body { bytes, final_url, etag, last_modified, .. } = fetch(client, &feed_url, "", "").await? else {
                return Err("Unexpected 304 on first fetch".into());
            };
            let p = feeds::parse(&bytes, &final_url).map_err(|e| e.to_string())?;
            Ok((final_url, p, etag, last_modified))
        }
    }
}

#[tauri::command]
pub fn feeds_list(state: State<'_, AppState>) -> Result<Vec<Feed>, String> {
    let conn = state.db.lock();
    feeds::ensure_tables(&conn).map_err(err)?;
    feeds::list_feeds(&conn).map_err(err)
}

/// Subscribe to a feed by its URL, or a website that advertises one.
#[tauri::command]
pub async fn feeds_add(state: State<'_, AppState>, url: String) -> Result<Feed, String> {
    let url = normalize_url(&url)?;
    let client = http()?;
    let (feed_url, parsed, etag, last_modified) = fetch_feed_document(&client, &url).await?;
    let conn = state.db.lock();
    feeds::ensure_tables(&conn).map_err(err)?;
    let now = db::now_ms();
    let id = feeds::insert_feed(&conn, &feed_url, now).map_err(err)?;
    feeds::apply_fetch(&conn, &id, &parsed, now).map_err(err)?;
    feeds::set_cache_headers(&conn, &id, &etag, &last_modified).map_err(err)?;
    feeds::list_feeds(&conn)
        .map_err(err)?
        .into_iter()
        .find(|f| f.id == id)
        .ok_or_else(|| "feed vanished after insert".into())
}

#[tauri::command]
pub fn feeds_remove(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();
    feeds::ensure_tables(&conn).map_err(err)?;
    feeds::remove_feed(&conn, &id).map_err(err)
}

/// Re-fetch one feed, or every feed when `id` is None. Conditional GET, so an
/// unchanged feed costs a 304. A failing feed records its error and the rest
/// still refresh. Returns the number of new items across all feeds.
#[tauri::command]
pub async fn feeds_refresh(state: State<'_, AppState>, id: Option<String>) -> Result<usize, String> {
    let targets: Vec<(String, String, String, String)> = {
        let conn = state.db.lock();
        feeds::ensure_tables(&conn).map_err(err)?;
        let ids: Vec<String> = match id {
            Some(i) => vec![i],
            None => feeds::list_feeds(&conn).map_err(err)?.into_iter().map(|f| f.id).collect(),
        };
        ids.into_iter()
            .filter_map(|i| feeds::cache_headers(&conn, &i).ok().map(|(u, e, l)| (i, u, e, l)))
            .collect()
    };
    refresh_all(&state, targets).await
}

pub(crate) async fn refresh_all(
    state: &AppState,
    targets: Vec<(String, String, String, String)>,
) -> Result<usize, String> {
    let client = http()?;
    let mut new = 0;
    for (id, url, etag, last_modified) in targets {
        let result = fetch(&client, &url, &etag, &last_modified).await;
        let conn = state.db.lock();
        let now = db::now_ms();
        match result {
            Ok(Fetched::NotModified) => {
                let _ = feeds::set_fetch_error(&conn, &id, "", now);
            }
            Ok(Fetched::Body { bytes, final_url, etag, last_modified, .. }) => match feeds::parse(&bytes, &final_url) {
                Ok(parsed) => {
                    new += feeds::apply_fetch(&conn, &id, &parsed, now).map_err(err)?;
                    let _ = feeds::set_cache_headers(&conn, &id, &etag, &last_modified);
                }
                Err(e) => {
                    let _ = feeds::set_fetch_error(&conn, &id, &e.to_string(), now);
                }
            },
            Err(e) => {
                let _ = feeds::set_fetch_error(&conn, &id, &e, now);
            }
        }
    }
    Ok(new)
}

#[tauri::command]
pub fn feed_items_list(
    state: State<'_, AppState>,
    feed_id: Option<String>,
    unread_only: Option<bool>,
    limit: Option<i64>,
) -> Result<Vec<FeedItem>, String> {
    let conn = state.db.lock();
    feeds::ensure_tables(&conn).map_err(err)?;
    feeds::list_items(&conn, feed_id.as_deref(), unread_only.unwrap_or(false), limit.unwrap_or(200).clamp(1, 1000))
        .map_err(err)
}

#[tauri::command]
pub fn feed_item_set_read(state: State<'_, AppState>, id: String, read: bool) -> Result<(), String> {
    let conn = state.db.lock();
    feeds::ensure_tables(&conn).map_err(err)?;
    feeds::set_read(&conn, &id, read).map_err(err)
}

#[tauri::command]
pub fn feed_items_mark_all_read(state: State<'_, AppState>, feed_id: Option<String>) -> Result<(), String> {
    let conn = state.db.lock();
    feeds::ensure_tables(&conn).map_err(err)?;
    feeds::mark_all_read(&conn, feed_id.as_deref()).map_err(err)
}

/// Subscribe to every feed in an OPML file. Feeds that fail are reported by
/// count, not fatal: an OPML export always carries a few dead subscriptions.
#[derive(serde::Serialize)]
pub struct OpmlImport {
    pub added: usize,
    pub failed: Vec<String>,
}

#[tauri::command]
pub async fn feeds_import_opml(state: State<'_, AppState>, path: String) -> Result<OpmlImport, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {path}: {e}"))?;
    let urls = feeds::opml_urls(&text);
    if urls.is_empty() {
        return Err("No feeds found in that file".into());
    }
    let client = http()?;
    let mut added = 0;
    let mut failed = Vec::new();
    for url in urls {
        match fetch_feed_document(&client, &url).await {
            Ok((feed_url, parsed, etag, last_modified)) => {
                let conn = state.db.lock();
                feeds::ensure_tables(&conn).map_err(err)?;
                let now = db::now_ms();
                let id = feeds::insert_feed(&conn, &feed_url, now).map_err(err)?;
                feeds::apply_fetch(&conn, &id, &parsed, now).map_err(err)?;
                let _ = feeds::set_cache_headers(&conn, &id, &etag, &last_modified);
                added += 1;
            }
            Err(_) => failed.push(url),
        }
    }
    Ok(OpmlImport { added, failed })
}

#[tauri::command]
pub fn feeds_export_opml(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let conn = state.db.lock();
    feeds::ensure_tables(&conn).map_err(err)?;
    let list = feeds::list_feeds(&conn).map_err(err)?;
    std::fs::write(&path, feeds::opml_export(&list)).map_err(|e| format!("Couldn't write {path}: {e}"))
}

// ─── Podcast episodes → notes ───────────────────────────────────────────────

/// An episode longer than this is not a podcast; refuse rather than fill the disk.
const MAX_EPISODE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Cached downloads older than this are swept on the next transcribe. The
/// import has copied what it needs into the note's recording by then.
const EPISODE_CACHE_TTL: Duration = Duration::from_secs(24 * 3600);
pub(crate) const PODCASTS_FOLDER: &str = "Podcasts";

/// File extension the import sidecar (AVFoundation) can decode for this
/// episode, from its URL first and its declared MIME type second. None means
/// a format we can't transcribe (Ogg/Opus, mostly).
pub(crate) fn episode_extension(url: &str, mime: &str) -> Option<&'static str> {
    const OK: [&str; 9] = ["mp3", "m4a", "m4b", "mp4", "aac", "wav", "caf", "aiff", "aif"];
    let path_ext = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.path().rsplit('.').next().map(|e| e.to_ascii_lowercase()))
        .filter(|e| e.len() <= 4);
    if let Some(e) = path_ext.as_deref() {
        if let Some(ok) = OK.iter().find(|o| **o == e) {
            return Some(ok);
        }
        if matches!(e, "ogg" | "oga" | "opus" | "webm" | "flac") {
            return None;
        }
    }
    let mime = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match mime.as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => Some("mp3"),
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" | "audio/x-m4b" => Some("m4a"),
        "video/mp4" => Some("mp4"),
        "audio/aac" | "audio/x-aac" | "audio/aacp" => Some("aac"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/aiff" | "audio/x-aiff" => Some("aiff"),
        // Most podcast hosts send audio/mpeg; an unknown audio type with no
        // telling extension is still most likely MP3 behind a redirect URL.
        "" | "application/octet-stream" | "binary/octet-stream" => Some("mp3"),
        m if m.starts_with("audio/") && !m.contains("ogg") && !m.contains("opus") && !m.contains("flac") => Some("mp3"),
        _ => None,
    }
}

#[derive(Clone, serde::Serialize)]
struct DownloadProgress {
    item_id: String,
    received: u64,
    total: u64,
}

fn sweep_episode_cache(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > EPISODE_CACHE_TTL);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Download to `<dest>.part`, then rename, so an interrupted download is never
/// mistaken for a finished one; a finished file is reused on retry (e.g. when
/// the first attempt found a recording in progress).
async fn download_episode(app: &AppHandle, item_id: &str, url: &str, dest: &std::path::Path) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }
    // No total timeout: a long episode on a slow link takes minutes. A stalled
    // connection is caught by the read timeout instead.
    let client = reqwest::Client::builder()
        .user_agent(concat!("Chatham/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(err)?;
    let resp = client.get(url).send().await.map_err(|e| format!("Couldn't download the episode: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("The episode host answered {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    if total > MAX_EPISODE_BYTES {
        return Err("That episode is larger than 2 GB — too big to transcribe here".into());
    }
    let part = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&part).await.map_err(err)?;
    let mut received: u64 = 0;
    let mut last_emit = std::time::Instant::now() - Duration::from_secs(1);
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download interrupted: {e}"))?;
        received += chunk.len() as u64;
        if received > MAX_EPISODE_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return Err("That episode is larger than 2 GB — too big to transcribe here".into());
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await.map_err(err)?;
        if last_emit.elapsed() >= Duration::from_millis(250) {
            last_emit = std::time::Instant::now();
            let _ = app.emit("feed_download_progress", DownloadProgress { item_id: item_id.into(), received, total });
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut file).await.map_err(err)?;
    drop(file);
    let _ = app.emit("feed_download_progress", DownloadProgress { item_id: item_id.into(), received, total: total.max(received) });
    tokio::fs::rename(&part, dest).await.map_err(err)?;
    Ok(())
}

/// The workspace's Podcasts folder, created the first time an episode lands.
pub(crate) fn podcasts_folder(conn: &rusqlite::Connection, workspace: &str) -> anyhow::Result<(db::Folder, bool)> {
    if let Some(f) = db::list_folders(conn, workspace)?
        .into_iter()
        .find(|f| f.name.eq_ignore_ascii_case(PODCASTS_FOLDER))
    {
        return Ok((f, false));
    }
    Ok((db::create_folder(conn, PODCASTS_FOLDER, workspace)?, true))
}

#[derive(serde::Serialize)]
pub struct TranscribedEpisode {
    pub note: db::Note,
    pub folder: db::Folder,
}

/// Download a podcast episode and transcribe it into a regular note, filed in
/// the Podcasts folder and titled after the episode. Rides the existing audio
/// import (`import_audio`), so the transcript fills in live, then diarizes and
/// summarises exactly like an imported recording. Returns once the import has
/// started; the frontend navigates to the note to watch it fill.
#[tauri::command]
pub async fn feed_item_transcribe(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    language: String,
    expected_speakers: Option<i64>,
) -> Result<TranscribedEpisode, String> {
    let item = {
        let conn = state.db.lock();
        feeds::ensure_tables(&conn).map_err(err)?;
        feeds::get_item(&conn, &id).map_err(|_| "That episode is no longer in your feeds".to_string())?
    };
    if item.enclosure_url.is_empty() {
        return Err("This item has no audio to transcribe".into());
    }
    let ext = episode_extension(&item.enclosure_url, &item.enclosure_type).ok_or_else(|| {
        "This episode's audio format (Ogg/Opus/FLAC) can't be transcribed yet".to_string()
    })?;

    let dir = app.path().app_cache_dir().map_err(err)?.join("podcasts");
    std::fs::create_dir_all(&dir).map_err(err)?;
    sweep_episode_cache(&dir);
    let dest = dir.join(format!("{}.{ext}", item.id));
    download_episode(&app, &item.id, &item.enclosure_url, &dest).await?;

    let mut note = super::import_audio(
        app.clone(),
        state.clone(),
        dest.to_string_lossy().into_owned(),
        language,
        expected_speakers,
    )
    .await?;

    let folder = {
        let conn = state.db.lock();
        let workspace = super::cloud::active_workspace(&conn);
        let (folder, created) = podcasts_folder(&conn, &workspace).map_err(err)?;
        if created {
            state.sync.folder_upserted(&folder.id);
        }
        // The episode title is user-meaningful, so the auto-titler leaves it
        // alone (`is_replaceable_title` only replaces placeholders).
        let title = if item.title.trim().is_empty() { "Podcast episode".to_string() } else { item.title.clone() };
        db::update_note(&conn, &note.id, &db::NotePatch { title: Some(title.clone()), ..Default::default() })
            .map_err(err)?;
        if item.published_at > 0 {
            db::set_note_created_at(&conn, &note.id, item.published_at).map_err(err)?;
            note.created_at = item.published_at;
        }
        db::move_note(&conn, &note.id, Some(&folder.id)).map_err(err)?;
        feeds::set_item_note(&conn, &item.id, &note.id).map_err(err)?;
        note.title = title;
        note.folder_id = Some(folder.id.clone());
        folder
    };
    state.sync.note_upserted(&note.id);
    Ok(TranscribedEpisode { note, folder })
}

#[cfg(test)]
mod tests {
    use super::{episode_extension, normalize_url, podcasts_folder};

    #[test]
    fn normalizes_addresses() {
        assert_eq!(normalize_url("example.com/feed").unwrap(), "https://example.com/feed");
        assert_eq!(normalize_url(" http://x.y/rss ").unwrap(), "http://x.y/rss");
        assert!(normalize_url("").is_err());
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("javascript:alert(1)").is_err());
    }

    #[test]
    fn picks_a_decodable_extension() {
        assert_eq!(episode_extension("https://cdn.x/ep.mp3?aid=rss", "audio/mpeg"), Some("mp3"));
        assert_eq!(episode_extension("https://cdn.x/ep.M4A", ""), Some("m4a"));
        // Tracking-redirect URLs carry no extension; fall back to the MIME type.
        assert_eq!(episode_extension("https://dts.podtrac.com/redirect/abc", "audio/x-m4a"), Some("m4a"));
        assert_eq!(episode_extension("https://h/e", "audio/mpeg"), Some("mp3"));
        assert_eq!(episode_extension("https://h/e", ""), Some("mp3"));
        assert_eq!(episode_extension("https://h/e.ogg", "audio/mpeg"), None, "the file wins over a wrong MIME");
        assert_eq!(episode_extension("https://h/e", "audio/ogg"), None);
        assert_eq!(episode_extension("https://h/e", "audio/opus"), None);
        assert_eq!(episode_extension("https://h/e", "text/html"), None);
    }

    #[test]
    fn podcasts_folder_is_created_once_and_found_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("podcasts.sqlite")).unwrap();
        let (a, created) = podcasts_folder(&conn, "").unwrap();
        assert!(created);
        assert_eq!(a.name, "Podcasts");
        let (b, created) = podcasts_folder(&conn, "").unwrap();
        assert!(!created);
        assert_eq!(a.id, b.id);
    }
}

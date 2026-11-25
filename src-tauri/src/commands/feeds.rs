//! Feeds (Labs) commands: subscribe, refresh, read, OPML. Storage and parsing
//! are `crate::feeds`; this module owns the network side and the Tauri surface.
//! See docs/design/feeds-labs.md.

use super::err;
use crate::feeds::{self, Feed, FeedItem};
use crate::{db, AppState};
use reqwest::header::{ACCEPT, CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use std::time::Duration;
use tauri::State;

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

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn normalizes_addresses() {
        assert_eq!(normalize_url("example.com/feed").unwrap(), "https://example.com/feed");
        assert_eq!(normalize_url(" http://x.y/rss ").unwrap(), "http://x.y/rss");
        assert!(normalize_url("").is_err());
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("javascript:alert(1)").is_err());
    }
}

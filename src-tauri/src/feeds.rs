//! Feeds (Labs, docs/design/feeds-labs.md): RSS/Atom/JSON-Feed subscriptions
//! and their items, plus OPML import/export. Pure parsing + SQLite here; the
//! Tauri commands and network I/O live in `commands/feeds.rs`.
//!
//! Local-only, like `inbox_triage`: nothing here syncs. Tables are created on
//! demand by `ensure_tables`, so there is no migration to coordinate.

use anyhow::{anyhow, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Feed {
    pub id: String,
    pub url: String,
    pub title: String,
    pub site_url: String,
    /// "rss" or "podcast" — podcast when any item carries an audio enclosure.
    pub kind: String,
    pub image_url: String,
    pub last_fetched_at: i64,
    pub error: String,
    pub unread: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FeedItem {
    pub id: String,
    pub feed_id: String,
    pub guid: String,
    pub title: String,
    pub link: String,
    pub author: String,
    pub published_at: i64,
    /// Sanitized HTML (see `sanitize_html`), safe to render.
    pub content_html: String,
    pub enclosure_url: String,
    pub enclosure_type: String,
    pub enclosure_bytes: i64,
    pub duration_s: i64,
    pub read: bool,
    /// The note an episode was transcribed into, once it has been.
    pub note_id: Option<String>,
    /// AI summary of an article (Markdown), once asked for. Episodes carry
    /// theirs on the transcribed note instead.
    pub summary: String,
}

/// What a fetch found, before it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFeed {
    pub title: String,
    pub site_url: String,
    pub image_url: String,
    pub items: Vec<ParsedItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedItem {
    pub guid: String,
    pub title: String,
    pub link: String,
    pub author: String,
    pub published_at: i64,
    pub content_html: String,
    pub enclosure_url: String,
    pub enclosure_type: String,
    pub enclosure_bytes: i64,
    pub duration_s: i64,
}

impl ParsedFeed {
    pub fn kind(&self) -> &'static str {
        if self.items.iter().any(|i| i.enclosure_type.starts_with("audio/")) {
            "podcast"
        } else {
            "rss"
        }
    }
}

pub fn ensure_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS feeds (
            id              TEXT PRIMARY KEY,
            url             TEXT NOT NULL UNIQUE,
            title           TEXT NOT NULL DEFAULT '',
            site_url        TEXT NOT NULL DEFAULT '',
            kind            TEXT NOT NULL DEFAULT 'rss',
            image_url       TEXT NOT NULL DEFAULT '',
            last_fetched_at INTEGER NOT NULL DEFAULT 0,
            etag            TEXT NOT NULL DEFAULT '',
            last_modified   TEXT NOT NULL DEFAULT '',
            error           TEXT NOT NULL DEFAULT '',
            created_at      INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS feed_items (
            id              TEXT PRIMARY KEY,
            feed_id         TEXT NOT NULL,
            guid            TEXT NOT NULL,
            title           TEXT NOT NULL DEFAULT '',
            link            TEXT NOT NULL DEFAULT '',
            author          TEXT NOT NULL DEFAULT '',
            published_at    INTEGER NOT NULL DEFAULT 0,
            content_html    TEXT NOT NULL DEFAULT '',
            enclosure_url   TEXT NOT NULL DEFAULT '',
            enclosure_type  TEXT NOT NULL DEFAULT '',
            enclosure_bytes INTEGER NOT NULL DEFAULT 0,
            duration_s      INTEGER NOT NULL DEFAULT 0,
            read            INTEGER NOT NULL DEFAULT 0,
            note_id         TEXT,
            UNIQUE(feed_id, guid)
        );
        CREATE INDEX IF NOT EXISTS idx_feed_items_feed ON feed_items(feed_id, published_at DESC);",
    )?;
    // Added in 0.81: tables created by 0.79/0.80 predate it.
    let has_summary: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('feed_items') WHERE name = 'summary'")?
        .exists([])?;
    if !has_summary {
        conn.execute("ALTER TABLE feed_items ADD COLUMN summary TEXT NOT NULL DEFAULT ''", [])?;
    }
    Ok(())
}

// ─── Parsing ─────────────────────────────────────────────────────────────────

/// Parse any RSS 0.9–2.0 / Atom / JSON Feed document (with iTunes podcast
/// extensions) into the shape we store. `base_url` resolves relative links.
pub fn parse(bytes: &[u8], base_url: &str) -> Result<ParsedFeed> {
    let feed = feed_rs::parser::Builder::new()
        .base_uri(Some(base_url))
        .build()
        .parse(bytes)
        .map_err(|e| anyhow!("not a feed: {e}"))?;

    let title = feed.title.as_ref().map(|t| clean_text(&t.content)).unwrap_or_default();
    let site_url = feed
        .links
        .iter()
        .find(|l| l.rel.as_deref() != Some("self"))
        .map(|l| l.href.clone())
        .unwrap_or_default();
    let image_url = feed
        .logo
        .as_ref()
        .map(|i| i.uri.clone())
        .or_else(|| feed.icon.as_ref().map(|i| i.uri.clone()))
        .unwrap_or_default();

    let items = feed
        .entries
        .into_iter()
        .map(|e| {
            let link = e
                .links
                .iter()
                .find(|l| l.rel.as_deref().map_or(true, |r| r == "alternate"))
                .or_else(|| e.links.first())
                .map(|l| l.href.clone())
                .unwrap_or_default();
            // A podcast episode's audio rides as a media object with content
            // (RSS <enclosure> / Atom rel=enclosure both land here).
            let enclosure = e
                .media
                .iter()
                .flat_map(|m| m.content.iter().map(move |c| (m, c)))
                .find(|(_, c)| c.url.is_some());
            let (enclosure_url, enclosure_type, enclosure_bytes, duration_s) = match enclosure {
                Some((m, c)) => (
                    c.url.as_ref().map(|u| u.to_string()).unwrap_or_default(),
                    c.content_type.as_ref().map(|t| t.to_string()).unwrap_or_default(),
                    c.size.map(|s| s as i64).unwrap_or(0),
                    c.duration
                        .or(m.duration)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                ),
                None => (String::new(), String::new(), 0, 0),
            };
            let body = e
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .or_else(|| e.summary.as_ref().map(|s| s.content.clone()))
                .or_else(|| {
                    e.media
                        .iter()
                        .find_map(|m| m.description.as_ref().map(|d| d.content.clone()))
                })
                .unwrap_or_default();
            let published_at = e
                .published
                .or(e.updated)
                .map(|d| d.timestamp_millis())
                .unwrap_or(0);
            // feed-rs synthesises an id when a feed omits one; fall back to the
            // link then the audio url so re-fetches upsert, never duplicate.
            let guid = if !e.id.trim().is_empty() {
                e.id.clone()
            } else if !link.is_empty() {
                link.clone()
            } else {
                enclosure_url.clone()
            };
            ParsedItem {
                guid,
                title: e.title.map(|t| clean_text(&t.content)).unwrap_or_default(),
                link,
                author: e.authors.first().map(|p| p.name.clone()).unwrap_or_default(),
                published_at,
                content_html: sanitize_html(&body),
                enclosure_url,
                enclosure_type,
                enclosure_bytes,
                duration_s,
            }
        })
        .collect();

    Ok(ParsedFeed { title, site_url, image_url, items })
}

/// Article HTML is untrusted. Keep structure and links; drop scripts, styles,
/// iframes, event handlers and — for privacy, matching Chatham's local-first
/// stance — every remote image. Links get rel=noopener noreferrer.
pub fn sanitize_html(html: &str) -> String {
    let mut b = ammonia::Builder::default();
    b.rm_tags(&["img", "picture", "source", "video", "audio", "iframe"]);
    b.link_rel(Some("noopener noreferrer"));
    b.clean(html).to_string()
}

fn clean_text(s: &str) -> String {
    // Titles sometimes carry entities or stray tags; strip to plain text.
    ammonia::Builder::empty().clean(s).to_string().replace("&amp;", "&").trim().to_string()
}

/// Find a feed URL advertised by an HTML page
/// (`<link rel="alternate" type="application/rss+xml" href="…">`).
pub fn discover_feed_link(html: &str, base_url: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find("<link") {
        let start = pos + i;
        let end = lower[start..].find('>').map(|e| start + e)?;
        let tag = &html[start..=end];
        let tl = &lower[start..=end];
        let is_alt = tl.contains("rel=\"alternate\"") || tl.contains("rel='alternate'") || tl.contains("rel=alternate");
        let is_feed = ["application/rss+xml", "application/atom+xml", "application/feed+json"]
            .iter()
            .any(|t| tl.contains(t));
        if is_alt && is_feed {
            if let Some(href) = attr(tag, "href") {
                return Some(resolve(base_url, &href));
            }
        }
        pos = end + 1;
    }
    None
}

fn attr(tag: &str, name: &str) -> Option<String> {
    // Attribute names are case-insensitive in HTML (and OPML writers vary on
    // `xmlUrl` vs `xmlurl`), so match on the lowercased tag with a lowercased
    // name, and require a word boundary so `href=` doesn't match `data-href=`.
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{}=", name.to_ascii_lowercase());
    let i = lower
        .match_indices(&needle)
        .find(|(i, _)| *i == 0 || lower.as_bytes()[i - 1].is_ascii_whitespace())
        .map(|(i, _)| i)?
        + needle.len();
    let rest = &tag[i..];
    let (q, rest) = match rest.chars().next()? {
        c @ ('"' | '\'') => (Some(c), &rest[1..]),
        _ => (None, rest),
    };
    let end = match q {
        Some(c) => rest.find(c)?,
        None => rest.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(rest.len()),
    };
    Some(rest[..end].replace("&amp;", "&"))
}

fn resolve(base: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    match reqwest::Url::parse(base).and_then(|b| b.join(href)) {
        Ok(u) => u.to_string(),
        Err(_) => href.to_string(),
    }
}

// ─── OPML ────────────────────────────────────────────────────────────────────

/// Every `xmlUrl` in an OPML document, in order, deduplicated.
pub fn opml_urls(opml: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, _) in opml.match_indices("<outline") {
        let end = opml[i..].find('>').map(|e| i + e).unwrap_or(opml.len() - 1);
        if let Some(u) = attr(&opml[i..=end], "xmlUrl") {
            let u = u.trim().to_string();
            if !u.is_empty() && !out.contains(&u) {
                out.push(u);
            }
        }
    }
    out
}

pub fn opml_export(feeds: &[Feed]) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    };
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head><title>Chatham feeds</title></head>\n  <body>\n",
    );
    for f in feeds {
        s.push_str(&format!(
            "    <outline type=\"rss\" text=\"{t}\" title=\"{t}\" xmlUrl=\"{u}\" htmlUrl=\"{h}\"/>\n",
            t = esc(&f.title),
            u = esc(&f.url),
            h = esc(&f.site_url)
        ));
    }
    s.push_str("  </body>\n</opml>\n");
    s
}

// ─── Storage ─────────────────────────────────────────────────────────────────

pub fn insert_feed(conn: &Connection, url: &str, now: i64) -> Result<String> {
    if let Some(id) = conn
        .query_row("SELECT id FROM feeds WHERE url = ?1", [url], |r| r.get::<_, String>(0))
        .optional()?
    {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute("INSERT INTO feeds (id, url, created_at) VALUES (?1, ?2, ?3)", params![id, url, now])?;
    Ok(id)
}

/// Store a fetch: feed metadata plus items, upserted by (feed_id, guid) so a
/// re-fetch never duplicates and never resets an item's read state or note.
/// Returns how many items were new.
pub fn apply_fetch(conn: &Connection, feed_id: &str, parsed: &ParsedFeed, now: i64) -> Result<usize> {
    conn.execute(
        "UPDATE feeds SET title = CASE WHEN ?2 = '' THEN title ELSE ?2 END,
                          site_url = ?3, image_url = ?4, kind = ?5,
                          last_fetched_at = ?6, error = ''
         WHERE id = ?1",
        params![feed_id, parsed.title, parsed.site_url, parsed.image_url, parsed.kind(), now],
    )?;
    let before = count_items(conn, feed_id)?;
    for it in &parsed.items {
        conn.execute(
            "INSERT INTO feed_items (id, feed_id, guid, title, link, author, published_at,
                                     content_html, enclosure_url, enclosure_type, enclosure_bytes, duration_s)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(feed_id, guid) DO UPDATE SET
               title = excluded.title, link = excluded.link, author = excluded.author,
               content_html = excluded.content_html, enclosure_url = excluded.enclosure_url,
               enclosure_type = excluded.enclosure_type, enclosure_bytes = excluded.enclosure_bytes,
               duration_s = excluded.duration_s",
            params![
                uuid::Uuid::new_v4().to_string(),
                feed_id,
                it.guid,
                it.title,
                it.link,
                it.author,
                it.published_at,
                it.content_html,
                it.enclosure_url,
                it.enclosure_type,
                it.enclosure_bytes,
                it.duration_s
            ],
        )?;
    }
    Ok((count_items(conn, feed_id)? - before).max(0) as usize)
}

pub fn set_fetch_error(conn: &Connection, feed_id: &str, error: &str, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE feeds SET error = ?2, last_fetched_at = ?3 WHERE id = ?1",
        params![feed_id, error, now],
    )?;
    Ok(())
}

pub fn set_cache_headers(conn: &Connection, feed_id: &str, etag: &str, last_modified: &str) -> Result<()> {
    conn.execute(
        "UPDATE feeds SET etag = ?2, last_modified = ?3 WHERE id = ?1",
        params![feed_id, etag, last_modified],
    )?;
    Ok(())
}

pub fn cache_headers(conn: &Connection, feed_id: &str) -> Result<(String, String, String)> {
    Ok(conn.query_row(
        "SELECT url, etag, last_modified FROM feeds WHERE id = ?1",
        [feed_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}

pub fn list_feeds(conn: &Connection) -> Result<Vec<Feed>> {
    let mut stmt = conn.prepare(
        "SELECT f.id, f.url, f.title, f.site_url, f.kind, f.image_url, f.last_fetched_at, f.error,
                (SELECT COUNT(*) FROM feed_items i WHERE i.feed_id = f.id AND i.read = 0)
         FROM feeds f ORDER BY f.title COLLATE NOCASE, f.url",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Feed {
                id: r.get(0)?,
                url: r.get(1)?,
                title: r.get(2)?,
                site_url: r.get(3)?,
                kind: r.get(4)?,
                image_url: r.get(5)?,
                last_fetched_at: r.get(6)?,
                error: r.get(7)?,
                unread: r.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn remove_feed(conn: &Connection, feed_id: &str) -> Result<()> {
    // Notes made from its episodes stay: they are the user's notes now.
    conn.execute("DELETE FROM feed_items WHERE feed_id = ?1", [feed_id])?;
    conn.execute("DELETE FROM feeds WHERE id = ?1", [feed_id])?;
    Ok(())
}

pub fn list_items(
    conn: &Connection,
    feed_id: Option<&str>,
    unread_only: bool,
    limit: i64,
) -> Result<Vec<FeedItem>> {
    let mut stmt = conn.prepare(
        "SELECT id, feed_id, guid, title, link, author, published_at, content_html,
                enclosure_url, enclosure_type, enclosure_bytes, duration_s, read, note_id, summary
         FROM feed_items
         WHERE (?1 IS NULL OR feed_id = ?1) AND (?2 = 0 OR read = 0)
         ORDER BY published_at DESC, rowid DESC
         LIMIT ?3",
    )?;
    let rows = stmt
        .query_map(params![feed_id, unread_only as i64, limit], map_item)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_item(conn: &Connection, id: &str) -> Result<FeedItem> {
    Ok(conn.query_row(
        "SELECT id, feed_id, guid, title, link, author, published_at, content_html,
                enclosure_url, enclosure_type, enclosure_bytes, duration_s, read, note_id, summary
         FROM feed_items WHERE id = ?1",
        [id],
        map_item,
    )?)
}

fn map_item(r: &rusqlite::Row) -> rusqlite::Result<FeedItem> {
    Ok(FeedItem {
        id: r.get(0)?,
        feed_id: r.get(1)?,
        guid: r.get(2)?,
        title: r.get(3)?,
        link: r.get(4)?,
        author: r.get(5)?,
        published_at: r.get(6)?,
        content_html: r.get(7)?,
        enclosure_url: r.get(8)?,
        enclosure_type: r.get(9)?,
        enclosure_bytes: r.get(10)?,
        duration_s: r.get(11)?,
        read: r.get::<_, i64>(12)? != 0,
        note_id: r.get(13)?,
        summary: r.get(14)?,
    })
}

pub fn set_item_summary(conn: &Connection, id: &str, summary: &str) -> Result<()> {
    conn.execute("UPDATE feed_items SET summary = ?2 WHERE id = ?1", params![id, summary])?;
    Ok(())
}

/// The readable text of an article page: the `<article>` (else `<main>`,
/// else `<body>`) with scripts, styles, navigation and chrome removed, as
/// plain text. Crude next to a real readability pass, but enough to give a
/// model the post when a feed only carries a teaser.
pub fn article_text_from_page(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let pick = |tag: &str| -> Option<&str> {
        let open = lower.find(&format!("<{tag}"))?;
        let close = lower.rfind(&format!("</{tag}>"))?;
        (close > open).then(|| &html[open..close])
    };
    let region = pick("article").or_else(|| pick("main")).or_else(|| pick("body")).unwrap_or(html);
    let mut b = ammonia::Builder::default();
    b.rm_tags(&["img", "picture", "source", "video", "audio", "iframe", "nav", "header", "footer", "aside", "form", "figure"]);
    b.clean_content_tags(std::collections::HashSet::from(["script", "style", "noscript", "svg", "nav", "header", "footer", "aside", "form", "button"]));
    let cleaned = b.clean(region).to_string();
    let text = crate::html_text::html_to_text(&cleaned);
    // Collapse the blank-line runs stripped chrome leaves behind.
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            blank += 1;
            if blank == 1 && !out.is_empty() {
                out.push('\n');
            }
        } else {
            blank = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

pub fn set_read(conn: &Connection, id: &str, read: bool) -> Result<()> {
    conn.execute("UPDATE feed_items SET read = ?2 WHERE id = ?1", params![id, read as i64])?;
    Ok(())
}

pub fn mark_all_read(conn: &Connection, feed_id: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE feed_items SET read = 1 WHERE (?1 IS NULL OR feed_id = ?1)",
        params![feed_id],
    )?;
    Ok(())
}

pub fn set_item_note(conn: &Connection, id: &str, note_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE feed_items SET note_id = ?2, read = 1 WHERE id = ?1",
        params![id, note_id],
    )?;
    Ok(())
}

pub fn count_items(conn: &Connection, feed_id: &str) -> Result<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM feed_items WHERE feed_id = ?1", [feed_id], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd">
<channel>
  <title>Data Talk &amp; More</title>
  <link>https://example.com/</link>
  <item>
    <title>Episode 2: Lakehouse</title>
    <guid>ep-2</guid>
    <link>https://example.com/ep2</link>
    <pubDate>Tue, 24 Nov 2026 10:00:00 GMT</pubDate>
    <description><![CDATA[<p>Show notes <img src="https://track.example/p.gif"> <script>alert(1)</script><a href="https://x.y" onclick="evil()">link</a></p>]]></description>
    <enclosure url="https://cdn.example.com/ep2.mp3" length="12345678" type="audio/mpeg"/>
    <itunes:duration>01:02:03</itunes:duration>
  </item>
  <item>
    <title>Episode 1</title>
    <guid>ep-1</guid>
    <pubDate>Mon, 23 Nov 2026 10:00:00 GMT</pubDate>
    <enclosure url="https://cdn.example.com/ep1.mp3" length="1000" type="audio/mpeg"/>
  </item>
</channel></rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>A Blog</title>
  <link href="https://blog.example/" rel="alternate"/>
  <link href="https://blog.example/feed.xml" rel="self"/>
  <entry>
    <title>Hello</title>
    <id>tag:blog.example,2026:1</id>
    <link href="/posts/hello" rel="alternate"/>
    <updated>2026-11-20T12:00:00Z</updated>
    <author><name>Ada</name></author>
    <content type="html">&lt;p&gt;Hi&lt;/p&gt;</content>
  </entry>
</feed>"#;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        ensure_tables(&c).unwrap();
        c
    }

    #[test]
    fn parses_a_podcast_feed_with_enclosures_and_duration() {
        let f = parse(RSS.as_bytes(), "https://example.com/feed").unwrap();
        assert_eq!(f.title, "Data Talk & More");
        assert_eq!(f.kind(), "podcast");
        assert_eq!(f.items.len(), 2);
        let ep = &f.items[0];
        assert_eq!(ep.guid, "ep-2");
        assert_eq!(ep.enclosure_url, "https://cdn.example.com/ep2.mp3");
        assert_eq!(ep.enclosure_type, "audio/mpeg");
        assert_eq!(ep.enclosure_bytes, 12_345_678);
        assert_eq!(ep.duration_s, 3723);
        assert!(ep.published_at > 0);
    }

    #[test]
    fn sanitizes_article_html() {
        let f = parse(RSS.as_bytes(), "https://example.com/feed").unwrap();
        let html = &f.items[0].content_html;
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("onclick"), "{html}");
        assert!(!html.contains("<img"), "tracking pixel must not load: {html}");
        assert!(html.contains("noopener"), "{html}");
        assert!(html.contains("Show notes"));
    }

    #[test]
    fn parses_atom_and_resolves_relative_links() {
        let f = parse(ATOM.as_bytes(), "https://blog.example/feed.xml").unwrap();
        assert_eq!(f.title, "A Blog");
        assert_eq!(f.kind(), "rss");
        assert_eq!(f.site_url, "https://blog.example/");
        let e = &f.items[0];
        assert_eq!(e.link, "https://blog.example/posts/hello");
        assert_eq!(e.author, "Ada");
        assert!(e.content_html.contains("<p>Hi</p>"));
    }

    #[test]
    fn rejects_non_feeds() {
        assert!(parse(b"<html><body>nope</body></html>", "https://x").is_err());
    }

    #[test]
    fn refetch_upserts_without_duplicating_or_resetting_state() {
        let c = db();
        let id = insert_feed(&c, "https://example.com/feed", 1).unwrap();
        assert_eq!(insert_feed(&c, "https://example.com/feed", 2).unwrap(), id, "same url, same feed");
        let f = parse(RSS.as_bytes(), "https://example.com/feed").unwrap();
        assert_eq!(apply_fetch(&c, &id, &f, 10).unwrap(), 2, "both items are new");
        let items = list_items(&c, Some(&id), false, 50).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].guid, "ep-2", "newest first");
        set_read(&c, &items[0].id, true).unwrap();
        set_item_note(&c, &items[1].id, "note-1").unwrap();

        assert_eq!(apply_fetch(&c, &id, &f, 20).unwrap(), 0, "nothing new on refetch");
        assert_eq!(count_items(&c, &id).unwrap(), 2);
        let again = list_items(&c, Some(&id), false, 50).unwrap();
        assert!(again[0].read, "read state survives a refetch");
        assert_eq!(again[1].note_id.as_deref(), Some("note-1"), "note link survives a refetch");

        let feeds = list_feeds(&c).unwrap();
        assert_eq!(feeds[0].kind, "podcast");
        assert_eq!(feeds[0].unread, 0, "one read, one transcribed");
    }

    #[test]
    fn unread_filter_and_mark_all_read() {
        let c = db();
        let id = insert_feed(&c, "u", 1).unwrap();
        apply_fetch(&c, &id, &parse(RSS.as_bytes(), "https://e").unwrap(), 1).unwrap();
        assert_eq!(list_items(&c, None, true, 50).unwrap().len(), 2);
        mark_all_read(&c, Some(&id)).unwrap();
        assert!(list_items(&c, None, true, 50).unwrap().is_empty());
    }

    #[test]
    fn removing_a_feed_drops_its_items() {
        let c = db();
        let id = insert_feed(&c, "u", 1).unwrap();
        apply_fetch(&c, &id, &parse(RSS.as_bytes(), "https://e").unwrap(), 1).unwrap();
        remove_feed(&c, &id).unwrap();
        assert!(list_feeds(&c).unwrap().is_empty());
        assert_eq!(count_items(&c, &id).unwrap(), 0);
    }

    #[test]
    fn summary_persists_across_refetch_and_old_tables_migrate() {
        let c = db();
        let id = insert_feed(&c, "u", 1).unwrap();
        let f = parse(RSS.as_bytes(), "https://e").unwrap();
        apply_fetch(&c, &id, &f, 1).unwrap();
        let item = &list_items(&c, None, false, 10).unwrap()[0];
        assert_eq!(item.summary, "");
        set_item_summary(&c, &item.id, "- point one").unwrap();
        apply_fetch(&c, &id, &f, 2).unwrap();
        assert_eq!(get_item(&c, &item.id).unwrap().summary, "- point one");

        // A 0.79 database: feed_items without the column.
        let old = Connection::open_in_memory().unwrap();
        old.execute_batch(
            "CREATE TABLE feed_items (id TEXT PRIMARY KEY, feed_id TEXT NOT NULL, guid TEXT NOT NULL,
               title TEXT NOT NULL DEFAULT '', link TEXT NOT NULL DEFAULT '', author TEXT NOT NULL DEFAULT '',
               published_at INTEGER NOT NULL DEFAULT 0, content_html TEXT NOT NULL DEFAULT '',
               enclosure_url TEXT NOT NULL DEFAULT '', enclosure_type TEXT NOT NULL DEFAULT '',
               enclosure_bytes INTEGER NOT NULL DEFAULT 0, duration_s INTEGER NOT NULL DEFAULT 0,
               read INTEGER NOT NULL DEFAULT 0, note_id TEXT, UNIQUE(feed_id, guid));
             INSERT INTO feed_items (id, feed_id, guid, title) VALUES ('x', 'f', 'g', 'Old');",
        )
        .unwrap();
        ensure_tables(&old).unwrap();
        ensure_tables(&old).unwrap(); // idempotent
        assert_eq!(get_item(&old, "x").unwrap().summary, "");
    }

    #[test]
    fn extracts_article_text_without_chrome() {
        let page = r#"<html><head><style>p{}</style><script>track()</script></head><body>
            <nav>Home | Blog</nav><header>Site</header>
            <article><h1>Lakebase in production</h1><p>First paragraph.</p>
            <script>ads()</script><aside>Related posts</aside><p>Second &amp; last.</p></article>
            <footer>© 2026</footer></body></html>"#;
        let t = article_text_from_page(page);
        assert!(t.contains("Lakebase in production"), "{t}");
        assert!(t.contains("First paragraph.") && t.contains("Second & last."), "{t}");
        for junk in ["track()", "ads()", "Related posts", "Home | Blog", "© 2026", "p{}"] {
            assert!(!t.contains(junk), "leaked {junk:?}: {t}");
        }
    }

    #[test]
    fn discovers_an_advertised_feed() {
        let html = r#"<html><head><link rel="stylesheet" href="/s.css"><link rel="alternate" type="application/rss+xml" title="RSS" href="/feed.xml?a=1&amp;b=2"></head></html>"#;
        assert_eq!(
            discover_feed_link(html, "https://site.example/blog/").as_deref(),
            Some("https://site.example/feed.xml?a=1&b=2")
        );
        assert_eq!(discover_feed_link("<html></html>", "https://x"), None);
    }

    #[test]
    fn opml_round_trip() {
        let opml = r#"<opml><body><outline text="Tech"><outline type="rss" text="A" xmlUrl="https://a/feed"/><outline type="rss" text="B" xmlUrl='https://b/rss'/></outline><outline xmlUrl="https://a/feed"/></body></opml>"#;
        assert_eq!(opml_urls(opml), vec!["https://a/feed", "https://b/rss"]);
        let feeds = vec![Feed {
            id: "1".into(),
            url: "https://a/feed?x=1&y=2".into(),
            title: "A & B".into(),
            site_url: "https://a".into(),
            kind: "rss".into(),
            image_url: String::new(),
            last_fetched_at: 0,
            error: String::new(),
            unread: 0,
        }];
        let out = opml_export(&feeds);
        assert!(out.contains("title=\"A &amp; B\""));
        assert_eq!(opml_urls(&out), vec!["https://a/feed?x=1&y=2"]);
    }
}

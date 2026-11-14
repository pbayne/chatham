//! Read Gmail inbox messages for triage (never sends, never deletes).
//!
//! Under the `gmail.readonly` scope we list unread INBOX messages and fetch
//! each one's headers + plain-text body. The body-extraction logic (walk the
//! MIME part tree, prefer text/plain, base64url-decode, strip to a budget) is
//! pure and unit-tested so triage quality doesn't depend on live Gmail.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, engine::general_purpose::URL_SAFE, Engine};
use serde::Serialize;
use serde_json::Value;

const LIST_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages";
const GET_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages";

/// How many characters of body we keep for the LLM. A triage decision needs
/// the gist, not the whole newsletter; capping keeps token cost (and latency
/// on a local model) bounded.
const BODY_CHAR_BUDGET: usize = 4000;

/// A fetched inbox message, flattened to what triage needs.
#[derive(Debug, Clone, Serialize)]
pub struct InboxMessage {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub snippet: String,
    pub body: String,
}

/// List unread INBOX message ids, newest first, capped at `max`.
pub async fn list_unread(
    http: &reqwest::Client,
    auth: &crate::gmail_auth::GmailAuth,
    max: u32,
) -> Result<Vec<String>> {
    let resp = auth
        .apply(http.get(LIST_URL))
        .query(&[
            ("q", "in:inbox is:unread"),
            ("maxResults", &max.to_string()),
        ])
        .send()
        .await
        .context("gmail messages.list request failed")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("gmail messages.list {status}: {text}"));
    }
    let v: Value = serde_json::from_str(&text).context("decode messages.list")?;
    Ok(v.get("messages")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|x| x.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// Fetch one message in `full` format and flatten it to an `InboxMessage`.
pub async fn fetch_message(
    http: &reqwest::Client,
    auth: &crate::gmail_auth::GmailAuth,
    id: &str,
) -> Result<InboxMessage> {
    let url = format!("{GET_URL}/{id}");
    let resp = auth
        .apply(http.get(&url))
        .query(&[("format", "full")])
        .send()
        .await
        .context("gmail messages.get request failed")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("gmail messages.get {status}: {text}"));
    }
    let v: Value = serde_json::from_str(&text).context("decode messages.get")?;
    Ok(flatten_message(id, &v))
}

/// Pure: turn a Gmail `users.messages.get` JSON payload into an `InboxMessage`.
/// Separated from the HTTP so the MIME walk + header extraction is unit-tested.
pub fn flatten_message(id: &str, v: &Value) -> InboxMessage {
    let thread_id = v
        .get("threadId")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let snippet = v
        .get("snippet")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let payload = v.get("payload");
    let header = |name: &str| -> String {
        payload
            .and_then(|p| p.get("headers"))
            .and_then(|h| h.as_array())
            .map(|arr| {
                arr.iter()
                    .find(|h| {
                        h.get("name")
                            .and_then(|n| n.as_str())
                            .map(|n| n.eq_ignore_ascii_case(name))
                            .unwrap_or(false)
                    })
                    .and_then(|h| h.get("value").and_then(|x| x.as_str()))
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_default()
    };
    let body_raw = payload.map(extract_body).unwrap_or_default();
    let body = truncate_chars(&body_raw, BODY_CHAR_BUDGET);
    InboxMessage {
        id: id.to_string(),
        thread_id,
        from: header("From"),
        to: header("To"),
        subject: header("Subject"),
        date: header("Date"),
        snippet,
        body,
    }
}

/// Walk a MIME payload tree and return the best plain-text body. Prefers
/// text/plain; falls back to a crude tag-strip of text/html; recurses into
/// multipart containers. Pure.
pub fn extract_body(payload: &Value) -> String {
    // Direct body on this part?
    let mime = payload
        .get("mimeType")
        .and_then(|m| m.as_str())
        .unwrap_or("");
    if mime == "text/plain" {
        if let Some(data) = part_data(payload) {
            return data;
        }
    }
    // Recurse into sub-parts, collecting the first text/plain we can find;
    // remember an html fallback in case no plain part exists anywhere.
    if let Some(parts) = payload.get("parts").and_then(|p| p.as_array()) {
        let mut html_fallback: Option<String> = None;
        for part in parts {
            let pm = part.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
            if pm.starts_with("multipart/") {
                let nested = extract_body(part);
                if !nested.is_empty() {
                    return nested;
                }
            } else if pm == "text/plain" {
                if let Some(d) = part_data(part) {
                    return d;
                }
            } else if pm == "text/html" && html_fallback.is_none() {
                if let Some(d) = part_data(part) {
                    html_fallback = Some(strip_html(&d));
                }
            }
        }
        if let Some(h) = html_fallback {
            return h;
        }
    }
    // Single-part html at the top level.
    if mime == "text/html" {
        if let Some(data) = part_data(payload) {
            return strip_html(&data);
        }
    }
    String::new()
}

/// Decode a part's `body.data` (base64url, Gmail sometimes pads with `=`).
fn part_data(part: &Value) -> Option<String> {
    let data = part.get("body")?.get("data")?.as_str()?;
    if data.is_empty() {
        return None;
    }
    // Gmail uses URL-safe base64; try no-pad then padded.
    let bytes = URL_SAFE_NO_PAD
        .decode(data.as_bytes())
        .or_else(|_| URL_SAFE.decode(data.as_bytes()))
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).to_string())
}

/// Very small HTML-to-text: drop tags, collapse whitespace. Good enough to let
/// the LLM read an HTML-only email; not a full renderer.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Collapse runs of whitespace to single spaces / newlines.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate to a character budget on a char boundary, adding an ellipsis note.
fn truncate_chars(s: &str, budget: usize) -> String {
    if s.chars().count() <= budget {
        return s.to_string();
    }
    let mut truncated: String = s.chars().take(budget).collect();
    truncated.push_str("\n…[truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn b64(s: &str) -> String {
        URL_SAFE_NO_PAD.encode(s.as_bytes())
    }

    #[test]
    fn extracts_plain_text_body() {
        let payload = json!({
            "mimeType": "text/plain",
            "body": { "data": b64("Hello, can you send the deck by Friday?") }
        });
        assert_eq!(
            extract_body(&payload),
            "Hello, can you send the deck by Friday?"
        );
    }

    #[test]
    fn prefers_plain_over_html_in_multipart() {
        let payload = json!({
            "mimeType": "multipart/alternative",
            "parts": [
                { "mimeType": "text/html", "body": { "data": b64("<p>ignored</p>") } },
                { "mimeType": "text/plain", "body": { "data": b64("the real body") } }
            ]
        });
        assert_eq!(extract_body(&payload), "the real body");
    }

    #[test]
    fn falls_back_to_html_stripped() {
        let payload = json!({
            "mimeType": "multipart/alternative",
            "parts": [
                { "mimeType": "text/html", "body": { "data": b64("<div>Hi <b>there</b></div>") } }
            ]
        });
        assert_eq!(extract_body(&payload), "Hi there");
    }

    #[test]
    fn recurses_nested_multipart() {
        let payload = json!({
            "mimeType": "multipart/mixed",
            "parts": [
                { "mimeType": "application/pdf", "body": { "attachmentId": "x" } },
                { "mimeType": "multipart/alternative", "parts": [
                    { "mimeType": "text/plain", "body": { "data": b64("nested body") } }
                ]}
            ]
        });
        assert_eq!(extract_body(&payload), "nested body");
    }

    #[test]
    fn flatten_pulls_headers_and_snippet() {
        let v = json!({
            "threadId": "t1",
            "snippet": "a snippet",
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    { "name": "From", "value": "Brad <brad@x.com>" },
                    { "name": "Subject", "value": "Deck" },
                    { "name": "Date", "value": "Mon, 1 Jan 2026 10:00:00 -0500" }
                ],
                "body": { "data": b64("body text") }
            }
        });
        let m = flatten_message("m1", &v);
        assert_eq!(m.id, "m1");
        assert_eq!(m.thread_id, "t1");
        assert_eq!(m.from, "Brad <brad@x.com>");
        assert_eq!(m.subject, "Deck");
        assert_eq!(m.snippet, "a snippet");
        assert_eq!(m.body, "body text");
    }

    #[test]
    fn truncates_long_body() {
        let long = "x".repeat(5000);
        let out = truncate_chars(&long, BODY_CHAR_BUDGET);
        assert!(out.ends_with("…[truncated]"));
        assert!(out.chars().count() < 5000);
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let v = json!({
            "payload": { "headers": [ { "name": "from", "value": "a@b.com" } ] }
        });
        let m = flatten_message("x", &v);
        assert_eq!(m.from, "a@b.com");
    }
}

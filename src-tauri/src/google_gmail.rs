//! Create a Gmail draft for an after-call follow-up (never auto-sends).
//!
//! The draft — recipients, subject, body — is assembled here as an RFC 2822
//! message, base64url-encoded, and handed to `users.drafts.create` under the
//! `gmail.compose` scope. It lands in the user's Drafts; they review and send
//! it themselves. This is a deliberate choice over `gmail.send`: a draft from a
//! transcript the model may have misheard must never leave without a human
//! reading it, and `gmail.compose` avoids Google's restricted-scope review.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};

const DRAFTS_CREATE: &str = "https://gmail.googleapis.com/gmail/v1/users/me/drafts";

/// The parts of a follow-up email.
#[derive(Debug, Clone)]
pub struct DraftSpec {
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// Fold a header value and strip CR/LF so a crafted title/attendee can't inject
/// extra headers (header-injection guard).
fn sanitize_header(s: &str) -> String {
    s.replace(['\r', '\n'], " ").trim().to_string()
}

/// Build the raw RFC 2822 message. Plain-text body, UTF-8. Pure + testable.
pub fn build_mime(spec: &DraftSpec) -> String {
    let to = spec
        .to
        .iter()
        .map(|a| sanitize_header(a))
        .filter(|a| !a.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let subject = sanitize_header(&spec.subject);
    // Body newlines normalized to CRLF per RFC 2822.
    let body = spec.body.replace("\r\n", "\n").replace('\n', "\r\n");
    format!(
        "To: {to}\r\n\
         Subject: {subject}\r\n\
         Content-Type: text/plain; charset=\"UTF-8\"\r\n\
         MIME-Version: 1.0\r\n\
         \r\n\
         {body}"
    )
}

/// base64url-encode the MIME message, as the Gmail API's `raw` field wants.
pub fn encode_raw(mime: &str) -> String {
    URL_SAFE_NO_PAD.encode(mime.as_bytes())
}

/// Create the draft. Returns the Gmail draft id and a deep link to it.
pub async fn create_draft(
    http: &reqwest::Client,
    access_token: &str,
    spec: &DraftSpec,
) -> Result<(String, String)> {
    let raw = encode_raw(&build_mime(spec));
    let resp = http
        .post(DRAFTS_CREATE)
        .bearer_auth(access_token)
        .json(&json!({ "message": { "raw": raw } }))
        .send()
        .await
        .context("gmail drafts.create request failed")?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("gmail drafts.create {status}: {body}"));
    }
    let v: Value = serde_json::from_str(&body).context("decode draft response")?;
    let draft_id = v
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    // Deep-link to the Drafts view (Gmail doesn't give a per-draft URL via API;
    // #drafts lands the user where their new draft is).
    let link = "https://mail.google.com/mail/u/0/#drafts".to_string();
    Ok((draft_id, link))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_has_headers_and_body() {
        let m = build_mime(&DraftSpec {
            to: vec!["a@x.com".into(), "b@y.com".into()],
            subject: "Follow-up: Weekly Sync".into(),
            body: "Hi all,\n\nAction items:\n- do X".into(),
        });
        assert!(m.contains("To: a@x.com, b@y.com\r\n"));
        assert!(m.contains("Subject: Follow-up: Weekly Sync\r\n"));
        assert!(m.contains("Content-Type: text/plain"));
        assert!(m.contains("Action items:\r\n- do X"));
    }

    #[test]
    fn header_injection_is_neutralized() {
        let m = build_mime(&DraftSpec {
            to: vec!["x@y.com\r\nBcc: evil@z.com".into()],
            subject: "hi\r\nX-Injected: 1".into(),
            body: "body".into(),
        });
        // The security property is that no NEW header LINE is created: the CRLF
        // in the attacker-controlled fields is flattened to a space, so the
        // injected text survives only as inert content on the To:/Subject:
        // lines, never as its own `\r\nBcc:` / `\r\nX-Injected:` header.
        assert!(!m.contains("\r\nBcc: evil@z.com"));
        assert!(!m.contains("\r\nX-Injected: 1"));
        // And the flattening did happen: the CR and LF each became a space, so
        // the addresses are joined by inert whitespace on one To: line.
        assert!(m.contains("To: x@y.com  Bcc: evil@z.com\r\n"));
    }

    #[test]
    fn raw_is_base64url_no_pad() {
        let r = encode_raw("To: a@b.com\r\n\r\nhi");
        assert!(!r.contains('+'));
        assert!(!r.contains('/'));
        assert!(!r.contains('='));
        // Round-trips.
        let decoded = URL_SAFE_NO_PAD.decode(r.as_bytes()).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "To: a@b.com\r\n\r\nhi");
    }

    #[test]
    fn empty_recipients_ok() {
        // A draft with no To is valid — the user fills it in before sending.
        let m = build_mime(&DraftSpec {
            to: vec![],
            subject: "s".into(),
            body: "b".into(),
        });
        assert!(m.contains("To: \r\n"));
    }
}

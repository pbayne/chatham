//! Google Calendar integration for calendar-driven auto-record ("Autopilot").
//!
//! See `docs/prd-calendar-autorecord.md` §6a. This is the Path B design:
//! talk to the Google Calendar API directly over HTTPS rather than reading
//! macOS EventKit, because the target users run Google Calendar without
//! mirroring it into macOS Internet Accounts, and because the API's richer
//! event metadata is the point — it gets attached to each recording's note.
//!
//! Auth: OAuth 2.0 Authorization Code + PKCE, loopback redirect (the installed
//! -app pattern). No client secret ships in the binary; PKCE replaces it. The
//! refresh token is stored in the Keychain via `crate::keychain` (the same
//! mechanism as provider API keys); the access token lives in memory and is
//! refreshed on demand.
//!
//! Scope: `calendar.events.readonly` — read-only, events only.
//!
//! STATUS: proof-of-concept scaffold. The OAuth flow, token refresh, and event
//! fetch are implemented and unit-tested at the pure-logic layer (PKCE, URL
//! building, event parsing). Wiring into `autopilot.rs` (the scheduler) and the
//! Tauri command surface is a follow-up milestone (M2+ in the PRD). Until that
//! wiring lands, the public surface is legitimately unused — silence the
//! dead-code lint module-wide rather than scatter per-item allows.
#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// OAuth client ID for the Humla Desktop-app OAuth client. Public by design for
/// installed apps (PKCE is what protects the flow, not a secret). Overridable at
/// build time via the GOOGLE_OAUTH_CLIENT_ID env var so the repo can ship without
/// a hard-coded project.
pub const CLIENT_ID: &str = match option_env!("GOOGLE_OAUTH_CLIENT_ID") {
    Some(id) => id,
    None => "REPLACE_ME.apps.googleusercontent.com",
};

/// OAuth scopes Humla requests. One consent screen covers every Google-backed
/// feature so the user connects once:
///   - calendar.events.readonly — Autopilot reads meetings (read-only)
///   - drive.file — create/own Docs Humla makes (NOT read existing Drive);
///     a non-sensitive scope, so no Google verification wall
///   - gmail.compose — create Gmail drafts for after-call follow-ups; drafts
///     are never auto-sent, so the user always reviews before sending
pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/calendar.events.readonly",
    "https://www.googleapis.com/auth/drive.file",
    "https://www.googleapis.com/auth/gmail.compose",
];

/// The space-joined scope string for the auth URL.
pub fn scope_param() -> String {
    SCOPES.join(" ")
}

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const CAL_BASE: &str = "https://www.googleapis.com/calendar/v3";

/// Keychain account under which the Google refresh token is stored.
pub const KEYCHAIN_ACCOUNT: &str = "google_calendar_refresh_token";

// ─── PKCE ────────────────────────────────────────────────────────────────────

/// A PKCE verifier/challenge pair. The verifier is the high-entropy secret kept
/// in memory; the challenge (its SHA-256, base64url) goes in the auth URL.
#[derive(Clone, Debug)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// Generate a fresh pair. Verifier is 64 url-safe chars (well within the
    /// RFC 7636 43–128 range); challenge is S256.
    pub fn generate() -> Self {
        use rand::Rng;
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
        let mut rng = rand::thread_rng();
        let verifier: String = (0..64)
            .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
            .collect();
        let challenge = Self::s256(&verifier);
        Self { verifier, challenge }
    }

    fn s256(verifier: &str) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(verifier.as_bytes());
        base64url(&digest)
    }
}

/// base64url without padding, per RFC 7636.
fn base64url(bytes: &[u8]) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    URL_SAFE_NO_PAD.encode(bytes)
}

// ─── Auth URL ──────────────────────────────────────────────────────────────

/// Build the consent URL the user opens in a browser. `redirect_uri` is the
/// loopback the one-shot listener bound to (e.g. `http://127.0.0.1:49721`);
/// `state` is an anti-CSRF nonce the caller verifies on the redirect.
pub fn auth_url(redirect_uri: &str, pkce: &Pkce, state: &str) -> String {
    let mut url = url::Url::parse(AUTH_ENDPOINT).expect("static auth endpoint parses");
    url.query_pairs_mut()
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &scope_param())
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("access_type", "offline") // ask for a refresh token
        .append_pair("prompt", "consent"); // force refresh-token issuance on re-auth
    url.to_string()
}

// ─── Token exchange + refresh ───────────────────���────────────────────────────

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// The live credential Autopilot holds in memory. `refresh_token` is also
/// persisted to the Keychain by the caller.
#[derive(Clone, Debug)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<std::time::Instant>,
}

impl Tokens {
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            // 60 s skew so we refresh just before the edge.
            Some(at) => std::time::Instant::now() + std::time::Duration::from_secs(60) >= at,
            None => true,
        }
    }
}

fn to_tokens(r: TokenResponse) -> Tokens {
    let expires_at = r
        .expires_in
        .map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    Tokens {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_at,
    }
}

/// Exchange the authorization `code` (from the loopback redirect) for tokens.
pub async fn exchange_code(
    client: &reqwest::Client,
    code: &str,
    redirect_uri: &str,
    pkce: &Pkce,
) -> Result<Tokens> {
    let resp = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", &pkce.verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .context("token exchange request failed")?;
    parse_token_response(resp).await
}

/// Mint a fresh access token from a stored refresh token.
pub async fn refresh_access_token(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<Tokens> {
    let resp = client
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .context("token refresh request failed")?;
    let mut tokens = parse_token_response(resp).await?;
    // Google omits the refresh token on refresh; keep the one we used.
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    Ok(tokens)
}

async fn parse_token_response(resp: reqwest::Response) -> Result<Tokens> {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // A 400 invalid_grant here means the refresh token was revoked/expired;
        // the caller should drop to a clean "reconnect Google" state.
        return Err(anyhow!("google token endpoint {status}: {body}"));
    }
    let parsed: TokenResponse =
        serde_json::from_str(&body).context("decode google token response")?;
    Ok(to_tokens(parsed))
}

// ─── Event model (the metadata we attach to notes) ───────────────────────────

/// A single calendar event, flattened to the fields Autopilot and the note
/// metadata care about. Mirrors the columns proposed in the PRD §6a.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct CalendarEvent {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    /// RFC3339 start/end. All-day events carry a date only; we skip those.
    pub start: Option<String>,
    pub end: Option<String>,
    pub organizer_email: Option<String>,
    pub attendees: Vec<Attendee>,
    /// Best meeting URL: conferenceData entry point first, then hangoutLink.
    pub meeting_url: Option<String>,
    pub html_link: Option<String>,
    pub status: Option<String>,
    /// This user's own response (accepted/declined/tentative/needsAction).
    pub self_response: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Attendee {
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub response_status: Option<String>,
    pub organizer: bool,
    pub is_self: bool,
}

impl CalendarEvent {
    /// Autopilot's recordable heuristic (PRD §3.2, refined by §6a): a timed,
    /// non-declined, non-cancelled event that is either a conference or has
    /// other attendees.
    pub fn is_recordable(&self) -> bool {
        if self.start.is_none() || self.end.is_none() {
            return false; // all-day / undated
        }
        if self.status.as_deref() == Some("cancelled") {
            return false;
        }
        if self.self_response.as_deref() == Some("declined") {
            return false;
        }
        let has_conf = self.meeting_url.is_some();
        let others = self.attendees.iter().filter(|a| !a.is_self).count();
        has_conf || others >= 1
    }

    /// Attendee display names, for seeding the Whisper custom-vocab trail so
    /// participant names transcribe correctly.
    pub fn attendee_names(&self) -> Vec<String> {
        self.attendees
            .iter()
            .filter_map(|a| a.display_name.clone())
            .collect()
    }
}

// ─── Event fetch ─────────────────────────────────────────────────────────────

/// Fetch events from one calendar in the `[time_min, time_max]` RFC3339 window.
/// `singleEvents=true` expands recurrences into concrete instances so the
/// scheduler sees real start times, and results are ordered by start.
pub async fn list_events(
    client: &reqwest::Client,
    access_token: &str,
    calendar_id: &str,
    time_min_rfc3339: &str,
    time_max_rfc3339: &str,
) -> Result<Vec<CalendarEvent>> {
    let url = format!(
        "{CAL_BASE}/calendars/{}/events",
        urlencode(calendar_id)
    );
    let resp = client
        .get(&url)
        .bearer_auth(access_token)
        .query(&[
            ("timeMin", time_min_rfc3339),
            ("timeMax", time_max_rfc3339),
            ("singleEvents", "true"),
            ("orderBy", "startTime"),
            ("maxResults", "50"),
        ])
        .send()
        .await
        .context("calendar events request failed")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("calendar events {status}: {body}"));
    }
    parse_events(&body, calendar_id)
}

/// Parse the Calendar API `events.list` JSON body into our flat model. Split
/// out from the HTTP call so it's unit-testable against fixture JSON.
pub fn parse_events(body: &str, calendar_id: &str) -> Result<Vec<CalendarEvent>> {
    let v: serde_json::Value = serde_json::from_str(body).context("decode events list")?;
    let items = v.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(items.len());
    for it in &items {
        out.push(parse_one_event(it, calendar_id));
    }
    Ok(out)
}

fn parse_one_event(it: &serde_json::Value, calendar_id: &str) -> CalendarEvent {
    let s = |k: &str| it.get(k).and_then(|x| x.as_str()).map(str::to_string);

    // start/end: prefer dateTime (timed); a date-only value marks all-day,
    // which we leave as-is here and let is_recordable() reject.
    let dt = |k: &str| {
        it.get(k)
            .and_then(|o| o.get("dateTime"))
            .and_then(|x| x.as_str())
            .map(str::to_string)
    };

    let organizer_email = it
        .get("organizer")
        .and_then(|o| o.get("email"))
        .and_then(|x| x.as_str())
        .map(str::to_string);

    let mut attendees = Vec::new();
    let mut self_response = None;
    if let Some(arr) = it.get("attendees").and_then(|a| a.as_array()) {
        for a in arr {
            let is_self = a.get("self").and_then(|x| x.as_bool()).unwrap_or(false);
            let response_status = a
                .get("responseStatus")
                .and_then(|x| x.as_str())
                .map(str::to_string);
            if is_self {
                self_response = response_status.clone();
            }
            attendees.push(Attendee {
                email: a.get("email").and_then(|x| x.as_str()).map(str::to_string),
                display_name: a
                    .get("displayName")
                    .and_then(|x| x.as_str())
                    .map(str::to_string),
                response_status,
                organizer: a.get("organizer").and_then(|x| x.as_bool()).unwrap_or(false),
                is_self,
            });
        }
    }

    // Meeting URL: a video entry point in conferenceData wins; fall back to
    // hangoutLink. Regexing location/notes is a last resort left to the caller.
    let meeting_url = conference_video_uri(it).or_else(|| s("hangoutLink"));

    CalendarEvent {
        id: s("id").unwrap_or_default(),
        calendar_id: calendar_id.to_string(),
        title: s("summary").unwrap_or_else(|| "(no title)".to_string()),
        description: s("description"),
        location: s("location"),
        start: dt("start"),
        end: dt("end"),
        organizer_email,
        attendees,
        meeting_url,
        html_link: s("htmlLink"),
        status: s("status"),
        self_response,
    }
}

fn conference_video_uri(it: &serde_json::Value) -> Option<String> {
    let eps = it
        .get("conferenceData")?
        .get("entryPoints")?
        .as_array()?;
    // Prefer a "video" entry point; otherwise take the first with a uri.
    eps.iter()
        .find(|e| e.get("entryPointType").and_then(|x| x.as_str()) == Some("video"))
        .or_else(|| eps.iter().find(|e| e.get("uri").is_some()))
        .and_then(|e| e.get("uri"))
        .and_then(|x| x.as_str())
        .map(str::to_string)
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

// ─── Tests (pure logic — no network) ─────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_base64url_of_verifier() {
        let p = Pkce::generate();
        assert!(p.verifier.len() >= 43 && p.verifier.len() <= 128);
        // Recompute the challenge independently and compare.
        assert_eq!(p.challenge, Pkce::s256(&p.verifier));
        // base64url: no '+', '/', or '=' padding.
        assert!(!p.challenge.contains('+'));
        assert!(!p.challenge.contains('/'));
        assert!(!p.challenge.contains('='));
    }

    #[test]
    fn pkce_known_vector() {
        // RFC 7636 Appendix B worked example.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let got = Pkce::s256(verifier);
        assert_eq!(got, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn auth_url_has_pkce_and_offline_access() {
        let pkce = Pkce {
            verifier: "v".into(),
            challenge: "chal".into(),
        };
        let u = auth_url("http://127.0.0.1:5000", &pkce, "nonce123");
        assert!(u.contains("code_challenge=chal"));
        assert!(u.contains("code_challenge_method=S256"));
        assert!(u.contains("access_type=offline"));
        assert!(u.contains("state=nonce123"));
        assert!(u.contains("response_type=code"));
        assert!(u.contains("scope=https"));
    }

    #[test]
    fn parses_a_conference_event_with_attendees() {
        let body = r#"{
          "items": [{
            "id": "evt1",
            "summary": "Weekly Sync",
            "description": "agenda",
            "status": "confirmed",
            "htmlLink": "https://calendar.google.com/evt1",
            "start": {"dateTime": "2026-11-13T15:00:00-05:00"},
            "end":   {"dateTime": "2026-11-13T15:30:00-05:00"},
            "organizer": {"email": "boss@databricks.com"},
            "attendees": [
              {"email": "patrick.bayne@databricks.com", "displayName": "Patrick Bayne", "responseStatus": "accepted", "self": true},
              {"email": "brad@databricks.com", "displayName": "Bradley Lucas", "responseStatus": "tentative"}
            ],
            "conferenceData": {"entryPoints": [
              {"entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij"}
            ]}
          }]
        }"#;
        let evs = parse_events(body, "primary").unwrap();
        assert_eq!(evs.len(), 1);
        let e = &evs[0];
        assert_eq!(e.title, "Weekly Sync");
        assert_eq!(e.organizer_email.as_deref(), Some("boss@databricks.com"));
        assert_eq!(e.meeting_url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert_eq!(e.self_response.as_deref(), Some("accepted"));
        assert_eq!(e.attendee_names(), vec!["Patrick Bayne", "Bradley Lucas"]);
        assert!(e.is_recordable());
    }

    #[test]
    fn declined_event_is_not_recordable() {
        let body = r#"{"items":[{
          "id":"e","summary":"Skip me","status":"confirmed",
          "start":{"dateTime":"2026-11-13T15:00:00-05:00"},
          "end":{"dateTime":"2026-11-13T15:30:00-05:00"},
          "attendees":[
            {"email":"patrick.bayne@databricks.com","responseStatus":"declined","self":true},
            {"email":"x@databricks.com","responseStatus":"accepted"}
          ]
        }]}"#;
        let e = &parse_events(body, "primary").unwrap()[0];
        assert!(!e.is_recordable());
    }

    #[test]
    fn all_day_event_is_not_recordable() {
        // date-only start/end → no dateTime → start/end None → rejected.
        let body = r#"{"items":[{
          "id":"e","summary":"Company holiday","status":"confirmed",
          "start":{"date":"2026-11-13"},"end":{"date":"2026-11-14"},
          "attendees":[{"email":"a@x.com"},{"email":"b@x.com"}]
        }]}"#;
        let e = &parse_events(body, "primary").unwrap()[0];
        assert_eq!(e.start, None);
        assert!(!e.is_recordable());
    }

    #[test]
    fn solo_event_without_conference_is_not_recordable() {
        let body = r#"{"items":[{
          "id":"e","summary":"Focus time","status":"confirmed",
          "start":{"dateTime":"2026-11-13T15:00:00-05:00"},
          "end":{"dateTime":"2026-11-13T16:00:00-05:00"},
          "attendees":[{"email":"patrick.bayne@databricks.com","self":true}]
        }]}"#;
        let e = &parse_events(body, "primary").unwrap()[0];
        assert!(!e.is_recordable());
    }
}

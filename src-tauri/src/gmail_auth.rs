//! Gmail authentication strategy.
//!
//! Parley reaches Gmail two ways, chosen by the `gmail_auth_mode` setting:
//!
//!   - `oauth` (default): the app's own OAuth refresh token on `Autopilot`.
//!     Requires a real Google OAuth client ID compiled in, and requires the
//!     signing account to be allowed to authorize third-party apps.
//!
//!   - `gcloud`: use the machine's existing Google Cloud Application Default
//!     Credentials — `gcloud auth application-default print-access-token` — plus
//!     the quota-project header. This is the route that works on a corp account
//!     whose admin policy blocks NEW third-party OAuth consents but has already
//!     blessed the user's own gcloud ADC (scope gmail.modify). No OAuth client,
//!     no consent screen, no policy exception. The token is minted fresh per
//!     call (the CLI caches + refreshes it), so nothing is stored by Parley.
//!
//! A resolved `GmailAuth` is just a bearer token plus the headers a request
//! needs, so the call sites in `gmail_read` / `google_gmail` don't branch on
//! mode — they ask for one and attach what it carries.

use anyhow::{anyhow, Context, Result};

/// The quota project every gcloud-ADC Google API call must bill to, or the
/// request 403s. Matches the Hermes CLI path (`x-goog-user-project`).
pub const QUOTA_PROJECT: &str = "gcp-dev-field-eng-aiapiquota";

const SETTING_MODE: &str = "gmail_auth_mode";

/// How Gmail calls authenticate. Parsed from the `gmail_auth_mode` setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GmailAuthMode {
    /// Parley's own OAuth refresh token (the Autopilot Google connection).
    Oauth,
    /// The machine's gcloud Application Default Credentials.
    Gcloud,
}

impl GmailAuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            GmailAuthMode::Oauth => "oauth",
            GmailAuthMode::Gcloud => "gcloud",
        }
    }

    pub fn parse(s: &str) -> GmailAuthMode {
        match s.trim().to_lowercase().as_str() {
            "gcloud" => GmailAuthMode::Gcloud,
            _ => GmailAuthMode::Oauth,
        }
    }
}

/// A resolved bearer token + the extra headers its mode requires. Call sites
/// attach these without knowing which mode produced them.
#[derive(Clone, Debug)]
pub struct GmailAuth {
    pub access_token: String,
    /// Extra headers (name, value) — e.g. the quota-project header for gcloud.
    pub extra_headers: Vec<(&'static str, String)>,
}

impl GmailAuth {
    /// Apply this auth to a request builder: bearer token + any extra headers.
    pub fn apply(&self, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb = rb.bearer_auth(&self.access_token);
        for (name, value) in &self.extra_headers {
            rb = rb.header(*name, value);
        }
        rb
    }
}

/// Read the configured Gmail auth mode from settings (default: oauth).
pub fn mode(conn: &rusqlite::Connection) -> GmailAuthMode {
    let raw = crate::db::get_setting(conn, SETTING_MODE)
        .ok()
        .flatten()
        .unwrap_or_default();
    GmailAuthMode::parse(&raw)
}

/// Mint a gcloud ADC access token by shelling out to the gcloud CLI. The CLI
/// owns caching + refresh, so this is cheap on the hot path and Parley never
/// persists the token. Errors clearly when gcloud is missing or ADC isn't set
/// up, so the UI can tell the user to run `gcloud auth application-default
/// login` rather than failing opaquely.
pub fn gcloud_access_token() -> Result<String> {
    let output = std::process::Command::new("gcloud")
        .args(["auth", "application-default", "print-access-token"])
        .output()
        .context("failed to run gcloud — is the Google Cloud SDK installed and on PATH?")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "gcloud could not provide an access token. Run `gcloud auth application-default login` first. ({})",
            stderr.trim()
        ));
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err(anyhow!("gcloud returned an empty access token"));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse_defaults_to_oauth() {
        assert_eq!(GmailAuthMode::parse(""), GmailAuthMode::Oauth);
        assert_eq!(GmailAuthMode::parse("oauth"), GmailAuthMode::Oauth);
        assert_eq!(GmailAuthMode::parse("nonsense"), GmailAuthMode::Oauth);
    }

    #[test]
    fn mode_parse_recognizes_gcloud() {
        assert_eq!(GmailAuthMode::parse("gcloud"), GmailAuthMode::Gcloud);
        assert_eq!(GmailAuthMode::parse("GCloud"), GmailAuthMode::Gcloud);
        assert_eq!(GmailAuthMode::parse(" gcloud "), GmailAuthMode::Gcloud);
    }

    #[test]
    fn round_trip_as_str() {
        for m in [GmailAuthMode::Oauth, GmailAuthMode::Gcloud] {
            assert_eq!(GmailAuthMode::parse(m.as_str()), m);
        }
    }

    #[test]
    fn apply_attaches_quota_header() {
        // Build a request and confirm the quota header rides along. We can't
        // read headers back off a RequestBuilder directly, so assert on the
        // built request via reqwest's clone+build.
        let client = reqwest::Client::new();
        let auth = GmailAuth {
            access_token: "tok".into(),
            extra_headers: vec![("x-goog-user-project", QUOTA_PROJECT.to_string())],
        };
        let rb = auth.apply(client.get("https://example.com"));
        let req = rb.build().expect("build request");
        assert_eq!(
            req.headers().get("x-goog-user-project").unwrap(),
            QUOTA_PROJECT
        );
        assert_eq!(
            req.headers().get("authorization").unwrap(),
            "Bearer tok"
        );
    }
}

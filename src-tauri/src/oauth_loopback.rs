//! One-shot loopback HTTP listener for the OAuth installed-app redirect.
//!
//! The PKCE flow (see `google_calendar`) sends the user to Google's consent
//! page with `redirect_uri = http://127.0.0.1:<port>`. After consent, Google
//! redirects the browser to that loopback with `?code=…&state=…`. This module
//! binds an ephemeral port, hands back the `redirect_uri` to put in the auth
//! URL, then blocks (with a timeout) for the single inbound request, parses the
//! query, verifies the anti-CSRF `state`, and returns the authorization code.
//!
//! Deliberately dependency-light: a raw `tokio` TCP accept + a minimal HTTP/1.1
//! response, rather than pulling a web framework in for one request. The only
//! thing served is a tiny "you can close this tab" page.

use anyhow::{anyhow, Result};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A bound loopback listener waiting for the OAuth redirect.
pub struct LoopbackServer {
    listener: TcpListener,
    redirect_uri: String,
}

impl LoopbackServer {
    /// Bind `127.0.0.1:0` (OS picks a free port). The resulting `redirect_uri`
    /// must be registered on the OAuth client as an allowed redirect — for
    /// "Desktop app" clients Google accepts any `http://127.0.0.1:<port>`, so
    /// no per-port registration is needed.
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| anyhow!("bind loopback: {e}"))?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}");
        Ok(Self { listener, redirect_uri })
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Wait for the browser redirect, verify `state`, return the auth `code`.
    /// Gives up after `timeout` so a user who abandons consent doesn't wedge
    /// the flow forever.
    pub async fn wait_for_code(self, expected_state: &str, timeout: Duration) -> Result<String> {
        let accept = async {
            loop {
                let (mut stream, _) = self
                    .listener
                    .accept()
                    .await
                    .map_err(|e| anyhow!("accept: {e}"))?;

                // Read just the request line + headers. The redirect is a GET,
                // so the first line carries everything we need.
                let mut buf = vec![0u8; 8192];
                let n = stream.read(&mut buf).await.map_err(|e| anyhow!("read: {e}"))?;
                let req = String::from_utf8_lossy(&buf[..n]);
                let Some(target) = request_target(&req) else {
                    respond(&mut stream, "Bad request").await;
                    continue; // not the redirect we want; keep listening
                };

                // Favicon or other noise — ignore and keep waiting.
                if target.starts_with("/favicon") {
                    respond(&mut stream, "").await;
                    continue;
                }

                let parsed = parse_redirect(&target, expected_state);
                match parsed {
                    Ok(code) => {
                        respond(
                            &mut stream,
                            "Humla is connected to Google Calendar. You can close this tab.",
                        )
                        .await;
                        return Ok(code);
                    }
                    Err(e) => {
                        respond(&mut stream, &format!("Authorization failed: {e}")).await;
                        return Err(e);
                    }
                }
            }
        };

        tokio::time::timeout(timeout, accept)
            .await
            .map_err(|_| anyhow!("timed out waiting for Google authorization"))?
    }
}

/// Pull the request target (path+query) out of the HTTP request line
/// `GET /?code=…&state=… HTTP/1.1`.
fn request_target(req: &str) -> Option<String> {
    let first = req.lines().next()?;
    let mut parts = first.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    Some(target.to_string())
}

/// Parse `/?code=…&state=…` (or `?error=…`), verifying the CSRF `state`.
/// Split out and pure so it's unit-testable without sockets.
fn parse_redirect(target: &str, expected_state: &str) -> Result<String> {
    // Build an absolute URL so the query parser has a base.
    let url = url::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|e| anyhow!("parse redirect url: {e}"))?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }

    if let Some(e) = error {
        return Err(anyhow!("google returned error: {e}"));
    }
    match state.as_deref() {
        Some(s) if s == expected_state => {}
        Some(_) => return Err(anyhow!("state mismatch (possible CSRF) — ignoring redirect")),
        None => return Err(anyhow!("redirect missing state")),
    }
    code.ok_or_else(|| anyhow!("redirect missing authorization code"))
}

/// Minimal HTTP/1.1 response with a tiny HTML body. Best-effort; errors writing
/// the courtesy page don't fail the flow.
async fn respond(stream: &mut tokio::net::TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><html><head><meta charset=utf-8>\
         <title>Humla</title></head><body style=\"font-family:-apple-system,\
         sans-serif;padding:3rem;text-align:center;color:#222\">\
         <h2>Humla</h2><p>{message}</p></body></html>"
    );
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_request_target() {
        let req = "GET /?code=abc&state=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(request_target(req).as_deref(), Some("/?code=abc&state=xyz"));
    }

    #[test]
    fn parses_code_when_state_matches() {
        let code = parse_redirect("/?code=auth_code_123&state=nonce", "nonce").unwrap();
        assert_eq!(code, "auth_code_123");
    }

    #[test]
    fn rejects_state_mismatch() {
        let err = parse_redirect("/?code=x&state=wrong", "nonce").unwrap_err();
        assert!(err.to_string().contains("state mismatch"));
    }

    #[test]
    fn surfaces_google_error() {
        let err = parse_redirect("/?error=access_denied&state=nonce", "nonce").unwrap_err();
        assert!(err.to_string().contains("access_denied"));
    }

    #[test]
    fn missing_code_is_an_error() {
        let err = parse_redirect("/?state=nonce", "nonce").unwrap_err();
        assert!(err.to_string().contains("missing authorization code"));
    }

    #[test]
    fn url_encoded_code_is_decoded() {
        // Google percent-encodes the code in the query; url crate decodes it.
        let code = parse_redirect("/?code=4%2F0Ab_c-d&state=n", "n").unwrap();
        assert_eq!(code, "4/0Ab_c-d");
    }
}

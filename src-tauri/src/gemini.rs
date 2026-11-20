//! Google Gemini's `generateContent` API — the summary transport for the
//! `gemini` provider. Modeled on `anthropic.rs`, but the wire shape is its
//! own: the key rides in an `x-goog-api-key` header, the system prompt is a
//! top-level `system_instruction` object, and the turns live under
//! `contents` with `parts[].text`. The reply text is at
//! `candidates[0].content.parts[].text`.
//!
//! Summaries collect the whole string anyway, so this takes the simpler
//! non-streaming `:generateContent` path and hands the finished text to the
//! caller as a single `StreamChunk::Content`, matching the callback contract
//! `anthropic::summarize` uses.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

pub const BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

/// A meeting summary is long-form; this ceiling matches the Anthropic path.
pub const SUMMARY_MAX_TOKENS: u32 = 16_000;

/// Default Gemini model for summaries. `gemini-2.5-flash` is a known real,
/// widely-deployed, stable model id — chosen deliberately over a newer-sounding
/// id that could not be verified against live Google docs from this build
/// environment. The user can change it in Settings → Summary at any time; the
/// stored `gemini_model` setting overrides this. If/when a newer Flash id is
/// confirmed live, bump this constant.
pub const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// Header carrying the API key. Gemini wants the key in `x-goog-api-key`
/// rather than a bearer token.
pub fn headers(key: &str) -> Vec<(&'static str, String)> {
    vec![("x-goog-api-key", key.to_string())]
}

/// Turn a non-2xx response into something a user can act on. The JSON body
/// carries `error.message`; the status is what says whose problem it is.
pub fn readable_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(|s| s.to_string()))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        400 if detail.to_lowercase().contains("api key") => {
            "Google rejected the API key — check it in Settings".to_string()
        }
        401 | 403 => "Google rejected the API key — check it in Settings".to_string(),
        429 => "Gemini rate limit hit — wait a moment and try again".to_string(),
        503 => "Gemini is overloaded right now — try again shortly".to_string(),
        s if (500..600).contains(&s) => format!("Gemini server error (HTTP {s}): {detail}"),
        s => format!("HTTP {s} from Gemini: {detail}"),
    }
}

/// Pull the reply text out of a `generateContent` response. The text is
/// split across `candidates[0].content.parts[].text`; concatenate every
/// part so a multi-part answer survives.
pub fn extract_text(body: &Value) -> String {
    body["candidates"][0]["content"]["parts"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// One-shot summary through `:generateContent`, handing the finished text to
/// `on_chunk` as a single `Content` delta. Non-streaming: the summary path
/// collects the whole string regardless, and the callback contract is the
/// same one `anthropic::summarize` satisfies.
pub async fn summarize<F>(
    api_key: &str,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    mut on_chunk: F,
) -> Result<String>
where
    F: FnMut(crate::openai::StreamChunk) + Send,
{
    let mut body = json!({
        "contents": [{
            "role": "user",
            "parts": [{ "text": user_message }],
        }],
        "generationConfig": {
            "maxOutputTokens": SUMMARY_MAX_TOKENS,
        },
    });
    if !system_prompt.trim().is_empty() {
        body["system_instruction"] = json!({ "parts": [{ "text": system_prompt }] });
    }

    let http = crate::openai::client();
    let url = format!("{BASE}/models/{model}:generateContent");
    let started = std::time::Instant::now();
    let make = || {
        let mut req = http.post(&url).json(&body);
        for (name, value) in headers(api_key) {
            req = req.header(name, value);
        }
        req
    };
    let r = match crate::openai::send_with_retries(make, started).await {
        Ok(resp) => resp,
        Err(e) if e.is_timeout() => {
            return Err(anyhow!(
                "Gemini timed out after {}s — try again.",
                started.elapsed().as_secs()
            ))
        }
        Err(e) => {
            return Err(anyhow!(
                "Could not reach Gemini: {}",
                crate::openai::error_chain(&e)
            ))
        }
    };

    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("{}", readable_error(status.as_u16(), &text)));
    }

    let parsed: Value = serde_json::from_str(&text)
        .map_err(|e| anyhow!("Gemini returned a response we couldn't parse: {e}"))?;
    let out = extract_text(&parsed);
    eprintln!(
        "[llm] gemini summary done in {:?}, {} chars",
        started.elapsed(),
        out.len()
    );
    if out.trim().is_empty() {
        return Err(anyhow!("{model} returned an empty response"));
    }
    on_chunk(crate::openai::StreamChunk::Content(&out));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_put_the_key_in_the_goog_header() {
        assert_eq!(headers("k"), vec![("x-goog-api-key", "k".to_string())]);
    }

    #[test]
    fn extract_text_joins_every_part() {
        let body = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "Hei " }, { "text": "der" }] }
            }]
        });
        assert_eq!(extract_text(&body), "Hei der");
    }

    #[test]
    fn extract_text_is_empty_when_there_are_no_candidates() {
        assert_eq!(extract_text(&json!({})), "");
        assert_eq!(extract_text(&json!({ "candidates": [] })), "");
    }

    #[test]
    fn readable_errors_name_the_cause() {
        let body = "{\"error\":{\"message\":\"boom\"}}";
        assert!(readable_error(403, body).contains("rejected the API key"));
        assert!(readable_error(429, body).contains("rate limit"));
        assert!(readable_error(503, body).contains("overloaded"));
        let five = readable_error(500, body);
        assert!(five.contains("server error"), "{five}");
        assert!(five.contains("boom"), "{five}");
        let other = readable_error(418, body);
        assert!(other.contains("HTTP 418"), "{other}");
        assert!(other.contains("boom"), "{other}");
    }

    #[test]
    fn a_bad_key_400_reads_as_a_key_problem() {
        let body = "{\"error\":{\"message\":\"API key not valid. Please pass a valid API key.\"}}";
        assert!(readable_error(400, body).contains("rejected the API key"));
    }

    #[test]
    fn an_unparseable_body_falls_back_to_a_snippet() {
        let msg = readable_error(400, "not json at all");
        assert!(msg.contains("not json at all"), "{msg}");
    }
}

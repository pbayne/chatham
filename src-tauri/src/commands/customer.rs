//! Automatic customer tagging (learns the account from the meeting).
//!
//! After a recording settles, a small model call reads the note and names the
//! customer organisation the meeting was with — e.g. "Fisher Investments",
//! "Fortress". The answer is matched (case-insensitive) against the existing
//! Clients; on a hit the note is tagged with that Client, on a miss a new
//! Client is created and the note tagged with it. So the account list builds
//! itself from what you actually record — no hardcoded seed list.
//!
//! Modelled on `title.rs` (a secondary, cheap, non-streaming LLM pass):
//!
//! - **Silent.** Failure is never surfaced. A user who never asked for a tag
//!   must not be toasted when the model is unconfigured or answers nothing.
//! - **Never overwrites a user's choice.** If the note already has a Client,
//!   the automatic path leaves it alone — only an untagged note is tagged.
//! - **Internal-meeting aware.** The model is told to answer NONE when the
//!   meeting isn't with an outside customer, so a standup doesn't invent one.

use super::summary::{note_language, resolve_provider, summary_provider_id};
use super::TITLE_FALLBACK_MODEL;
use crate::db;
use crate::openai;
use crate::AppState;
use tauri::{AppHandle, Emitter, Manager, State};

/// Cap on a customer name the model returns. A real org name is short; past
/// this the model wrote prose, so reject rather than tag with a sentence.
const NAME_MAX_CHARS: usize = 60;

/// The sentinel the prompt asks for when the meeting has no outside customer.
const NONE_SENTINEL: &str = "NONE";

/// How much transcript the extraction sees. The customer is usually named in
/// the opening (introductions), so the head is the right part — cheaper than
/// the title path's full-length sampling.
const TRANSCRIPT_CHAR_BUDGET: usize = 2_500;
const NOTES_CHAR_BUDGET: usize = 500;
const SUMMARY_CHAR_BUDGET: usize = 1_000;

/// Clean the model's answer into a customer name, or `None` when it declined
/// (the NONE sentinel) or returned junk.
pub(crate) fn clean_customer(raw: &str) -> Option<String> {
    let mut t: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    // Peel label prefixes + wrapping quotes + trailing period, like clean_title.
    loop {
        let before = t.clone();
        for label in ["customer:", "client:", "company:", "organisation:", "organization:"] {
            if t.len() > label.len()
                && t.chars().zip(label.chars()).all(|(a, b)| a.to_ascii_lowercase() == b)
            {
                t = t[label.len()..].trim_start().to_string();
            }
        }
        t = strip_wrapping_quotes(&t).to_string();
        t = t.trim_end_matches(['.', '\u{2026}']).trim().to_string();
        if t == before {
            break;
        }
    }
    let trimmed = t.trim();
    if trimmed.is_empty() || !trimmed.chars().any(char::is_alphanumeric) {
        return None;
    }
    // The model said there's no outside customer.
    if trimmed.eq_ignore_ascii_case(NONE_SENTINEL) || trimmed.eq_ignore_ascii_case("none.") {
        return None;
    }
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return None;
    }
    Some(trimmed.to_string())
}

fn strip_wrapping_quotes(t: &str) -> &str {
    const PAIRS: [(char, char); 5] = [
        ('"', '"'),
        ('\'', '\''),
        ('\u{201c}', '\u{201d}'),
        ('\u{00ab}', '\u{00bb}'),
        ('\u{2018}', '\u{2019}'),
    ];
    let mut chars = t.chars();
    let (Some(first), Some(last)) = (chars.next(), chars.next_back()) else {
        return t;
    };
    for (open, close) in PAIRS {
        if first == open && last == close {
            return t[open.len_utf8()..t.len() - close.len_utf8()].trim();
        }
    }
    t
}

fn head_chars(text: &str, budget: usize) -> String {
    let t = text.trim();
    let cut = t.char_indices().nth(budget).map_or(t.len(), |(i, _)| i);
    t[..cut].trim_end().to_string()
}

/// The blocks the model reads to name the customer.
pub(crate) fn customer_input(body_text: &str, transcript: &str, summary: &str) -> String {
    let notes = head_chars(body_text, NOTES_CHAR_BUDGET);
    let spoken = head_chars(transcript, TRANSCRIPT_CHAR_BUDGET);
    let notes_block = if notes.is_empty() { "(none)".to_string() } else { notes };
    let transcript_block = if spoken.is_empty() { "(none)".to_string() } else { spoken };
    let mut input = format!("[Notes]\n{notes_block}\n\n[Transcript]\n{transcript_block}");
    let summary = head_chars(summary, SUMMARY_CHAR_BUDGET);
    if !summary.is_empty() {
        input.push_str(&format!("\n\n[Summary]\n{summary}"));
    }
    input
}

/// The extraction prompt. Deliberately tight: name the outside customer org, or
/// answer NONE. No reasoning, no explanation — a small model given latitude
/// here invents a vendor from a passing mention.
fn customer_prompt() -> String {
    format!(
        "You identify the customer organisation a meeting was with. Read the note \
         below and answer with the name of the external customer / client company \
         the meeting is about — for example a company name like \"Fisher Investments\" \
         or \"Fortress\". Answer with the company name ALONE: no quotes, no label, no \
         explanation. If the meeting is internal, a personal note, or has no clear \
         outside customer, answer exactly {NONE_SENTINEL}. Do not guess from a brand \
         mentioned only in passing."
    )
}

/// Tag `note_id` with its customer if it has none yet — spawned, not awaited,
/// from the post-summary chain. Swallows every error (see module docs).
pub(crate) async fn auto_tag_customer(app: AppHandle, note_id: String) {
    if let Err(e) = try_auto_tag(&app, &note_id).await {
        eprintln!("[customer] note {note_id}: {e:#}");
    }
}

async fn try_auto_tag(app: &AppHandle, note_id: &str) -> anyhow::Result<()> {
    let state: State<AppState> = app.state();

    // Only untagged notes, and only if auto-tagging is on (default on). Resolve
    // the provider first so an unconfigured local user costs nothing.
    let provider_id = {
        let conn = state.db.lock();
        if !is_enabled(&conn) {
            return Ok(());
        }
        let n = db::get_note(&conn, note_id)?;
        if n.client_id.is_some() {
            return Ok(()); // already tagged — never overwrite
        }
        summary_provider_id(&conn, &n)?
    };
    let api_key = match provider_id.keychain_account() {
        Some(_) => super::read_provider_api_key(&state, provider_id.as_str())
            .map_err(|e| anyhow::anyhow!("{e}"))?,
        None => None,
    };
    let (mut provider, _language, note, workspace) = {
        let conn = state.db.lock();
        let n = db::get_note(&conn, note_id)?;
        if n.client_id.is_some() {
            return Ok(());
        }
        let p = resolve_provider(&conn, &n, api_key)?;
        let ws = super::cloud::active_workspace(&conn);
        (p, note_language(&conn, &n)?, n, ws)
    };

    let body_text = crate::html_text::html_to_text(&note.body);
    if body_text.trim().is_empty() && note.transcript.trim().is_empty() {
        return Ok(());
    }

    // Thinking is waste on a one-name extraction.
    provider.think = false;
    if provider.base_url == openai::BASE && openai::is_reasoning_model(&provider.model) {
        provider.model = TITLE_FALLBACK_MODEL.to_string();
    }

    let raw = provider
        .complete(&customer_prompt(), &customer_input(&body_text, &note.transcript, &note.summary), |_| {})
        .await?;
    let Some(name) = clean_customer(&raw) else {
        return Ok(()); // model declined or returned junk — leave untagged
    };

    // Match an existing Client case-insensitively, else create one. Re-check the
    // note is still untagged under the lock (the user may have tagged it while
    // the model worked — their choice wins).
    let client_id = {
        let conn = state.db.lock();
        let current = db::get_note(&conn, note_id)?;
        if current.client_id.is_some() {
            return Ok(());
        }
        let existing = db::list_clients(&conn, &workspace)?
            .into_iter()
            .find(|c| c.name.trim().eq_ignore_ascii_case(name.trim()));
        let client = match existing {
            Some(c) => c,
            None => db::create_client(&conn, &name, &workspace)?,
        };
        db::set_note_client(&conn, note_id, Some(&client.id))?;
        client.id
    };

    state.sync.note_upserted(note_id);
    let _ = app.emit("notes_changed", ());
    // If Obsidian sync is on, re-file the note now that it has a customer — it
    // moves into the customer subfolder. Best-effort; never fails the tag.
    {
        let conn = state.db.lock();
        super::obsidian::sync_one_best_effort(&conn, note_id);
    }
    eprintln!("[customer] note {note_id}: tagged client {client_id} ({name:?})");
    Ok(())
}

/// Whether auto customer-tagging is on. Default ON — it's additive and only
/// touches untagged notes, and the user asked for it to just work.
fn is_enabled(conn: &rusqlite::Connection) -> bool {
    db::get_setting(conn, "auto_customer_tagging")
        .ok()
        .flatten()
        .map(|v| v != "false" && v != "0")
        .unwrap_or(true)
}

#[tauri::command]
pub fn auto_customer_tagging_status(state: State<AppState>) -> Result<bool, String> {
    let conn = state.db.lock();
    Ok(is_enabled(&conn))
}

#[tauri::command]
pub fn auto_customer_tagging_set(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let conn = state.db.lock();
    db::set_setting(&conn, "auto_customer_tagging", if enabled { "true" } else { "false" })
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_plain_company_name() {
        assert_eq!(clean_customer("Fisher Investments").as_deref(), Some("Fisher Investments"));
        assert_eq!(clean_customer("Fortress").as_deref(), Some("Fortress"));
    }

    #[test]
    fn strips_label_quotes_and_period() {
        assert_eq!(clean_customer("Customer: Fisher Investments.").as_deref(), Some("Fisher Investments"));
        assert_eq!(clean_customer("\"Fortress\"").as_deref(), Some("Fortress"));
        assert_eq!(clean_customer("Company: “Acme Corp”").as_deref(), Some("Acme Corp"));
    }

    #[test]
    fn none_sentinel_means_no_tag() {
        assert_eq!(clean_customer("NONE"), None);
        assert_eq!(clean_customer("none"), None);
        assert_eq!(clean_customer("None."), None);
    }

    #[test]
    fn junk_and_prose_are_rejected() {
        assert_eq!(clean_customer(""), None);
        assert_eq!(clean_customer("   "), None);
        assert_eq!(clean_customer("\"\""), None);
        let prose = "The meeting appears to be with a financial services firm, likely \
                     one of several the team has been speaking with over the past quarter about";
        assert_eq!(clean_customer(prose), None);
    }

    #[test]
    fn input_blocks_are_labelled() {
        let i = customer_input("agenda", "Hi, this is Fisher Investments", "");
        assert!(i.contains("[Notes]\nagenda"));
        assert!(i.contains("[Transcript]\nHi, this is Fisher Investments"));
        assert!(!i.contains("[Summary]"));
        let withsum = customer_input("", "spoken", "a summary");
        assert!(withsum.contains("[Summary]\na summary"));
    }
}

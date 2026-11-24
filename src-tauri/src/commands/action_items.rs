//! Action items / follow-ups (#followups).
//!
//! Two halves:
//!   1. **Auto-extraction** — after a recording is summarized, a small model
//!      pass pulls the concrete action items out of the note (text + owner),
//!      so the user has a running to-do list without typing one. Runs ONCE per
//!      note (guarded by `note_has_auto_action_items`) so re-summarizing or
//!      switching templates doesn't duplicate or clobber items the user has
//!      since edited. Silent, like the customer + title passes.
//!   2. **CRUD + cross-note list** — the per-note panel and the Follow-ups view
//!      read/write through these commands.

use super::summary::{note_language, resolve_provider, summary_provider_id};
use super::TITLE_FALLBACK_MODEL;
use crate::db::{self, ActionItem, OpenActionItem};
use crate::openai;
use crate::AppState;
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, State};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

const TRANSCRIPT_CHAR_BUDGET: usize = 6_000;
const NOTES_CHAR_BUDGET: usize = 1_500;
const SUMMARY_CHAR_BUDGET: usize = 3_000;
/// Cap on extracted items — a meeting with 40 "action items" is a model that
/// turned the whole transcript into bullets; keep the top handful.
const MAX_ITEMS: usize = 12;

// ─── CRUD + list commands ────────────────────────────────────────────────────

#[tauri::command]
pub fn action_items_list(state: State<AppState>, note_id: String) -> Result<Vec<ActionItem>, String> {
    let conn = state.db.lock();
    db::list_action_items(&conn, &note_id).map_err(err)
}

/// Every open action item across the active workspace, for the Follow-ups view.
#[tauri::command]
pub fn action_items_open(state: State<AppState>) -> Result<Vec<OpenActionItem>, String> {
    let conn = state.db.lock();
    let workspace = super::cloud::active_workspace(&conn);
    db::list_open_action_items(&conn, &workspace).map_err(err)
}

#[tauri::command]
pub fn action_items_create(
    state: State<AppState>,
    note_id: String,
    text: String,
    owner: Option<String>,
    due: Option<String>,
) -> Result<ActionItem, String> {
    let conn = state.db.lock();
    db::create_action_item(&conn, &note_id, &text, owner.as_deref().unwrap_or(""), due.as_deref().unwrap_or(""), "user")
        .map_err(err)
}

#[tauri::command]
pub fn action_items_update(
    state: State<AppState>,
    id: String,
    text: Option<String>,
    owner: Option<String>,
    due: Option<String>,
    done: Option<bool>,
) -> Result<ActionItem, String> {
    let conn = state.db.lock();
    db::update_action_item(&conn, &id, text.as_deref(), owner.as_deref(), due.as_deref(), done).map_err(err)
}

#[tauri::command]
pub fn action_items_delete(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();
    db::delete_action_item(&conn, &id).map_err(err)
}

// ─── Auto-extraction ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ExtractedItem {
    #[serde(default)]
    text: String,
    #[serde(default)]
    owner: String,
    #[serde(default)]
    due: String,
}

/// Normalize a due value from the model into an ISO date (YYYY-MM-DD) when it's
/// a relative term we can resolve against the meeting's date, else pass through
/// (an explicit "2026-03-14" is kept; an unparseable phrase like "end of
/// quarter" is kept verbatim so the user still sees the intent). `base_ms` is
/// the note's created_at epoch-ms. Pure + deterministic — the LLM proposes the
/// phrase, this does the date math, never the model.
fn normalize_due(due: &str, base_ms: i64) -> String {
    use chrono::Datelike;
    let d = due.trim();
    if d.is_empty() {
        return String::new();
    }
    // Already an ISO date → keep as-is.
    if is_iso_date(d) {
        return d.to_string();
    }
    let base = match chrono::DateTime::from_timestamp_millis(base_ms) {
        Some(dt) => dt.date_naive(),
        None => return d.to_string(),
    };
    let lower = d.to_ascii_lowercase();
    let add = |days: i64| (base + chrono::Duration::days(days)).format("%Y-%m-%d").to_string();
    match lower.as_str() {
        "today" => add(0),
        "tomorrow" => add(1),
        "next week" => add(7),
        "end of week" | "end of the week" => {
            // Friday of the current week (Mon=0 … Sun=6).
            let dow = base.weekday().num_days_from_monday() as i64;
            add((4 - dow).rem_euclid(7))
        }
        _ => {
            // A weekday name → the NEXT occurrence strictly after base.
            if let Some(target) = weekday_index(&lower) {
                let cur = base.weekday().num_days_from_monday() as i64;
                let delta = (target - cur).rem_euclid(7);
                // Same weekday as the base rolls forward a full week (the next
                // occurrence strictly after today), not to today itself.
                add(if delta == 0 { 7 } else { delta })
            } else {
                // Unresolvable phrase ("end of month", "Q2") — keep the words.
                d.to_string()
            }
        }
    }
}

fn is_iso_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Mon=0 … Sun=6 for a lowercase weekday word (full or 3-letter), else None.
fn weekday_index(w: &str) -> Option<i64> {
    let w = w.trim_end_matches(|c| c == '.' || c == ',');
    Some(match w {
        "monday" | "mon" => 0,
        "tuesday" | "tue" | "tues" => 1,
        "wednesday" | "wed" => 2,
        "thursday" | "thu" | "thurs" => 3,
        "friday" | "fri" => 4,
        "saturday" | "sat" => 5,
        "sunday" | "sun" => 6,
        _ => return None,
    })
}

fn head_chars(text: &str, budget: usize) -> String {
    let t = text.trim();
    let cut = t.char_indices().nth(budget).map_or(t.len(), |(i, _)| i);
    t[..cut].trim_end().to_string()
}

fn extract_input(body_text: &str, transcript: &str, summary: &str) -> String {
    let notes = head_chars(body_text, NOTES_CHAR_BUDGET);
    let spoken = head_chars(transcript, TRANSCRIPT_CHAR_BUDGET);
    let mut input = format!(
        "[Notes]\n{}\n\n[Transcript]\n{}",
        if notes.is_empty() { "(none)" } else { &notes },
        if spoken.is_empty() { "(none)" } else { &spoken }
    );
    let summary = head_chars(summary, SUMMARY_CHAR_BUDGET);
    if !summary.is_empty() {
        input.push_str(&format!("\n\n[Summary]\n{summary}"));
    }
    input
}

fn extract_prompt() -> String {
    "You extract concrete action items and follow-ups from a meeting note. \
     Return ONLY a JSON array (no prose, no code fence) of objects with keys \
     \"text\" (the action, imperative, one line), \"owner\" (the person \
     responsible if clearly stated, else an empty string), and \"due\" (when \
     it's due if a time is stated — a plain date like \"2026-03-14\" if an \
     explicit calendar date is given, otherwise the words used such as \
     \"Friday\", \"next week\", \"end of month\"; empty string if no deadline \
     was mentioned). Include only real, specific commitments and next steps — \
     not general discussion. Do not invent owners or dates. If there are none, \
     return []. Example: \
     [{\"text\":\"Send the pricing proposal to Fisher\",\"owner\":\"Patrick\",\"due\":\"Friday\"}]"
        .to_string()
}

/// Pull a JSON array out of the model's answer, tolerating a code fence or
/// surrounding prose.
fn parse_items(raw: &str) -> Vec<ExtractedItem> {
    let trimmed = raw.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    let start = trimmed.find('[');
    let end = trimmed.rfind(']');
    let slice = match (start, end) {
        (Some(s), Some(e)) if e > s => &trimmed[s..=e],
        _ => return Vec::new(),
    };
    serde_json::from_str::<Vec<ExtractedItem>>(slice)
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.text.trim().is_empty())
        .take(MAX_ITEMS)
        .collect()
}

/// Extract action items for `note_id` once — spawned from the post-summary
/// chain. No-op if the note already has auto items (the user owns them after).
/// Swallows errors (silent, like the customer pass).
pub(crate) async fn auto_extract(app: AppHandle, note_id: String) {
    if let Err(e) = try_extract(&app, &note_id).await {
        eprintln!("[action-items] note {note_id}: {e:#}");
    }
}

async fn try_extract(app: &AppHandle, note_id: &str) -> anyhow::Result<()> {
    let state: State<AppState> = app.state();
    let provider_id = {
        let conn = state.db.lock();
        if db::note_has_auto_action_items(&conn, note_id)? {
            return Ok(()); // extracted once already — don't clobber user edits
        }
        let n = db::get_note(&conn, note_id)?;
        summary_provider_id(&conn, &n)?
    };
    let api_key = match provider_id.keychain_account() {
        Some(_) => super::read_provider_api_key(&state, provider_id.as_str()).map_err(|e| anyhow::anyhow!("{e}"))?,
        None => None,
    };
    let (mut provider, _language, note) = {
        let conn = state.db.lock();
        let n = db::get_note(&conn, note_id)?;
        let p = resolve_provider(&conn, &n, api_key)?;
        (p, note_language(&conn, &n)?, n)
    };
    let body_text = crate::html_text::html_to_text(&note.body);
    if body_text.trim().is_empty() && note.transcript.trim().is_empty() {
        return Ok(());
    }
    provider.think = false;
    if provider.base_url == openai::BASE && openai::is_reasoning_model(&provider.model) {
        provider.model = TITLE_FALLBACK_MODEL.to_string();
    }
    let raw = provider.complete(&extract_prompt(), &extract_input(&body_text, &note.transcript, &note.summary), |_| {}).await?;
    let items = parse_items(&raw);
    if items.is_empty() {
        return Ok(());
    }
    {
        let conn = state.db.lock();
        // Re-check under the lock — a concurrent pass or the user may have added
        // items while the model worked.
        if db::note_has_auto_action_items(&conn, note_id)? {
            return Ok(());
        }
        for it in &items {
            let due = normalize_due(&it.due, note.created_at);
            let _ = db::create_action_item(&conn, note_id, it.text.trim(), it.owner.trim(), &due, "auto");
        }
    }
    let _ = app.emit("action_items_changed", note_id.to_string());
    eprintln!("[action-items] note {note_id}: extracted {} item(s)", items.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_json_array() {
        let items = parse_items(r#"[{"text":"Send proposal","owner":"Pat"},{"text":"Book follow-up","owner":""}]"#);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].text, "Send proposal");
        assert_eq!(items[0].owner, "Pat");
    }

    #[test]
    fn tolerates_code_fence_and_prose() {
        let raw = "Here are the items:\n```json\n[{\"text\":\"Email the deck\"}]\n```";
        let items = parse_items(raw);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "Email the deck");
    }

    #[test]
    fn empty_and_junk_yield_nothing() {
        assert!(parse_items("[]").is_empty());
        assert!(parse_items("no items here").is_empty());
        assert!(parse_items("").is_empty());
        // blank-text items are filtered
        assert!(parse_items(r#"[{"text":"  ","owner":"x"}]"#).is_empty());
    }

    #[test]
    fn caps_at_max_items() {
        let many: String = "[".to_string()
            + &(0..30).map(|i| format!("{{\"text\":\"item {i}\"}}")).collect::<Vec<_>>().join(",")
            + "]";
        assert_eq!(parse_items(&many).len(), MAX_ITEMS);
    }

    #[test]
    fn parses_the_due_field() {
        let items = parse_items(r#"[{"text":"Send SOW","owner":"Sarah","due":"Friday"}]"#);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].due, "Friday");
        // Missing due tolerated (serde default "").
        let no_due = parse_items(r#"[{"text":"Call back","owner":""}]"#);
        assert_eq!(no_due[0].due, "");
    }

    // 2026-03-11 00:00 UTC is a Wednesday; use it as the meeting date base.
    const WED_2026_03_11_MS: i64 = 1_773_187_200_000;

    #[test]
    fn normalize_due_keeps_iso_and_empty() {
        assert_eq!(normalize_due("", WED_2026_03_11_MS), "");
        assert_eq!(normalize_due("2026-03-14", WED_2026_03_11_MS), "2026-03-14");
    }

    #[test]
    fn normalize_due_resolves_today_tomorrow() {
        assert_eq!(normalize_due("today", WED_2026_03_11_MS), "2026-03-11");
        assert_eq!(normalize_due("Tomorrow", WED_2026_03_11_MS), "2026-03-12");
        assert_eq!(normalize_due("next week", WED_2026_03_11_MS), "2026-03-18");
    }

    #[test]
    fn normalize_due_resolves_next_weekday() {
        // From Wed, "Friday" → the 13th; "Monday" → next Monday the 16th.
        assert_eq!(normalize_due("Friday", WED_2026_03_11_MS), "2026-03-13");
        assert_eq!(normalize_due("mon", WED_2026_03_11_MS), "2026-03-16");
        // Same weekday as base rolls forward a week (strictly after), not today.
        assert_eq!(normalize_due("Wednesday", WED_2026_03_11_MS), "2026-03-18");
    }

    #[test]
    fn normalize_due_end_of_week_is_friday() {
        assert_eq!(normalize_due("end of week", WED_2026_03_11_MS), "2026-03-13");
    }

    #[test]
    fn normalize_due_keeps_unresolvable_phrase() {
        assert_eq!(normalize_due("end of month", WED_2026_03_11_MS), "end of month");
        assert_eq!(normalize_due("Q2", WED_2026_03_11_MS), "Q2");
    }
}

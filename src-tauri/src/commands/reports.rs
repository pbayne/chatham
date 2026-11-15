//! Tauri commands for multiple reports per note (#reports).
//!
//! Read/delete the template-tagged reports a note has accumulated. Reports are
//! created by the summarize write-back (one per preset), so there's no create
//! command here — you generate a report by picking a template and summarizing.
//!
//! Also hosts template SUGGESTION: a small model pass that reads the transcript
//! and proposes which call-type template fits, so the user doesn't have to pick
//! blind.

use super::summary::{note_language, resolve_provider, summary_provider_id};
use super::TITLE_FALLBACK_MODEL;
use crate::db::{self, NoteReport};
use crate::openai;
use crate::AppState;
use tauri::{AppHandle, Manager, State};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// All reports for a note, newest first.
#[tauri::command]
pub fn note_reports_list(state: State<AppState>, note_id: String) -> Result<Vec<NoteReport>, String> {
    let conn = state.db.lock();
    db::list_note_reports(&conn, &note_id).map_err(err)
}

/// Delete one report. If it was the note's active summary, fall the note's
/// `summary` column back to the most recent remaining report (or empty), so the
/// legacy summary view never points at a deleted report.
#[tauri::command]
pub fn note_reports_delete(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();
    let report = db::get_note_report(&conn, &id).map_err(err)?;
    db::delete_note_report(&conn, &id).map_err(err)?;
    // Keep notes.summary coherent with what's left.
    let remaining = db::list_note_reports(&conn, &report.note_id).map_err(err)?;
    let next_summary = remaining.first().map(|r| r.content.clone()).unwrap_or_default();
    let next_preset = remaining
        .first()
        .map(|r| r.preset.clone())
        .unwrap_or_else(|| "meeting".to_string());
    db::update_note(
        &conn,
        &report.note_id,
        &db::NotePatch {
            summary: Some(next_summary),
            summary_preset: Some(next_preset),
            ..Default::default()
        },
    )
    .map_err(err)?;
    Ok(())
}

/// Make a stored report the note's active summary (its `summary` column), so
/// the main summary view + exports show it. Switching reports in the UI calls
/// this so the choice persists.
#[tauri::command]
pub fn note_reports_set_active(state: State<AppState>, id: String) -> Result<(), String> {
    let conn = state.db.lock();
    let report = db::get_note_report(&conn, &id).map_err(err)?;
    db::update_note(
        &conn,
        &report.note_id,
        &db::NotePatch {
            summary: Some(report.content.clone()),
            summary_preset: Some(report.preset.clone()),
            ..Default::default()
        },
    )
    .map_err(err)?;
    Ok(())
}

/// The preset values the suggester is allowed to return — the built-in call
/// types. A custom prompt isn't suggestable (the model can't know the user's
/// private prompts), so suggestion ranges over these.
const SUGGESTABLE: &[&str] = &[
    "meeting",
    "one_on_one",
    "lecture",
    "interview",
    "brainstorm",
    "voice_memo",
    "sales_call",
    "discovery_call",
    "technical_review",
    "qbr",
    "standup",
];

/// Map a model's free-text answer to a known preset value, or None. Tolerant of
/// casing, spaces/hyphens vs underscores, and the model echoing the label
/// ("Sales call") instead of the value ("sales_call").
pub(crate) fn match_preset(raw: &str) -> Option<String> {
    let norm = |s: &str| -> String {
        s.trim()
            .to_lowercase()
            .chars()
            .map(|c| if c == ' ' || c == '-' { '_' } else { c })
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect()
    };
    let answer = norm(raw);
    if answer.is_empty() {
        return None;
    }
    for v in SUGGESTABLE {
        if answer == norm(v) || answer == norm(&crate::presets::label_for(v)) {
            return Some((*v).to_string());
        }
    }
    for v in SUGGESTABLE {
        if answer.contains(&norm(v)) {
            return Some((*v).to_string());
        }
    }
    None
}

fn suggest_prompt() -> String {
    let list = SUGGESTABLE
        .iter()
        .map(|v| format!("{v} ({})", crate::presets::label_for(v)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "You classify what kind of call a transcript is, to pick a note template. \
         Read the note below and answer with EXACTLY ONE of these template values, \
         and nothing else — no explanation, no punctuation: {list}. \
         If unsure, answer meeting."
    )
}

/// Suggest a template (preset value) for a note from its content. Returns the
/// preset value, or `None` when there's nothing to go on. A button in the UI
/// calls this, so it reports errors (unlike the silent auto passes).
#[tauri::command]
pub async fn suggest_template(app: AppHandle, note_id: String) -> Result<Option<String>, String> {
    try_suggest(&app, &note_id).await.map_err(|e| e.to_string())
}

async fn try_suggest(app: &AppHandle, note_id: &str) -> anyhow::Result<Option<String>> {
    let state: State<AppState> = app.state();
    let provider_id = {
        let conn = state.db.lock();
        let n = db::get_note(&conn, note_id)?;
        summary_provider_id(&conn, &n)?
    };
    let api_key = match provider_id.keychain_account() {
        Some(_) => super::read_provider_api_key(&state, provider_id.as_str())
            .map_err(|e| anyhow::anyhow!("{e}"))?,
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
        return Ok(None);
    }
    provider.think = false;
    if provider.base_url == openai::BASE && openai::is_reasoning_model(&provider.model) {
        provider.model = TITLE_FALLBACK_MODEL.to_string();
    }
    let input = super::customer::customer_input(&body_text, &note.transcript, &note.summary);
    let raw = provider.complete(&suggest_prompt(), &input, |_| {}).await?;
    Ok(match_preset(&raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_value_label_and_loose() {
        assert_eq!(match_preset("sales_call").as_deref(), Some("sales_call"));
        assert_eq!(match_preset("Sales call").as_deref(), Some("sales_call"));
        assert_eq!(match_preset("  QBR  ").as_deref(), Some("qbr"));
        assert_eq!(match_preset("I think this is a discovery-call").as_deref(), Some("discovery_call"));
        assert_eq!(match_preset("Quarterly business review").as_deref(), Some("qbr"));
    }

    #[test]
    fn unknown_is_none() {
        assert_eq!(match_preset(""), None);
        assert_eq!(match_preset("potato"), None);
    }
}

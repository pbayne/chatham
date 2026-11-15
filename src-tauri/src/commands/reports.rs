//! Tauri commands for multiple reports per note (#reports).
//!
//! Read/delete the template-tagged reports a note has accumulated. Reports are
//! created by the summarize write-back (one per preset), so there's no create
//! command here — you generate a report by picking a template and summarizing.

use crate::db::{self, NoteReport};
use crate::AppState;
use tauri::State;

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

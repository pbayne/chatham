//! Tauri commands for Obsidian vault sync (obsidian.rs).
//!
//! The Settings panel reads the status, toggles the feature, picks the vault
//! folder, and triggers a one-off backfill. The note lifecycle (save/delete)
//! calls `crate::obsidian::sync_note` / `remove_note` directly elsewhere.

use crate::obsidian;
use crate::{db, AppState};
use serde::Serialize;
use tauri::State;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
pub struct ObsidianStatus {
    pub enabled: bool,
    pub vault_path: String,
    /// Count of notes currently tracked as synced (best-effort; 0 when the
    /// state file can't be read).
    pub synced_count: usize,
    pub conflict_count: usize,
}

#[tauri::command]
pub fn obsidian_status(state: State<AppState>) -> Result<ObsidianStatus, String> {
    let conn = state.db.lock();
    let (enabled, vault_path) = obsidian::config(&conn);
    let (synced_count, conflict_count) = obsidian::counts(&vault_path);
    Ok(ObsidianStatus { enabled, vault_path, synced_count, conflict_count })
}

/// Set the vault folder. Does not enable sync by itself. Triggers a backfill if
/// sync is already on.
#[tauri::command]
pub fn obsidian_set_vault(state: State<AppState>, path: String) -> Result<usize, String> {
    let conn = state.db.lock();
    db::set_setting(&conn, "obsidian_vault_path", path.trim()).map_err(err)?;
    if obsidian::is_active(&conn) {
        backfill_now(&conn)
    } else {
        Ok(0)
    }
}

/// Enable/disable sync. On enable (with a vault set), backfill every note.
#[tauri::command]
pub fn obsidian_set_enabled(state: State<AppState>, enabled: bool) -> Result<usize, String> {
    let conn = state.db.lock();
    db::set_setting(&conn, "obsidian_sync_enabled", if enabled { "true" } else { "false" })
        .map_err(err)?;
    if enabled && obsidian::is_active(&conn) {
        backfill_now(&conn)
    } else {
        Ok(0)
    }
}

/// Force a full re-export (every live note). Returns how many were written.
#[tauri::command]
pub fn obsidian_backfill(state: State<AppState>) -> Result<usize, String> {
    let conn = state.db.lock();
    backfill_now(&conn)
}

fn backfill_now(conn: &rusqlite::Connection) -> Result<usize, String> {
    let workspace = crate::commands::cloud::active_workspace(conn);
    let notes = db::list_notes(conn, &workspace).map_err(err)?;
    let folders = db::list_folders(conn, &workspace).map_err(err)?;
    Ok(obsidian::backfill(conn, &notes, &folders))
}

/// Best-effort: mirror one note to the vault after a save. Called from the note
/// lifecycle (notes_update, summary completion). Never errors into the caller —
/// a vault write must not break a note save. No-op when sync is off.
pub(crate) fn sync_one_best_effort(conn: &rusqlite::Connection, note_id: &str) {
    if !obsidian::is_active(conn) {
        return;
    }
    let Ok(note) = db::get_note(conn, note_id) else {
        return;
    };
    if note.deleted_at.is_some() {
        return;
    }
    let workspace = crate::commands::cloud::active_workspace(conn);
    let folders = db::list_folders(conn, &workspace).unwrap_or_default();
    let _ = obsidian::sync_note(conn, &note, &folders);
}

/// Best-effort: remove one note's vault copy after a delete.
pub(crate) fn remove_one_best_effort(conn: &rusqlite::Connection, note_id: &str) {
    if !obsidian::is_active(conn) {
        return;
    }
    let _ = obsidian::remove_note(conn, note_id);
}

//! Tauri commands for meeting captures (#captures): list and delete the
//! screenshots + clipboard snippets attached to a note while it was recording.
//!
//! The capture *engine* (watchers, inserts) lives in `crate::captures` and is
//! driven by `recording_start`/`recording_stop`. These commands are the
//! read/delete surface the Note view uses.
//!
//! Image rows store a path relative to the note's recordings dir; we resolve it
//! to an absolute path here so the frontend can load it via `convertFileSrc`.
//! Text rows carry the snippet inline.

use crate::{sessions, AppState};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// One capture shown in the Note view's Captures strip.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    /// "image" | "text"
    pub kind: String,
    /// Absolute filesystem path for images (empty for text).
    pub path: String,
    /// Snippet text for clipboard-text captures (empty for images).
    pub text: String,
    /// Milliseconds into the recording when captured (for ordering).
    pub offset_ms: i64,
    /// Unix-ms creation time.
    pub created_at: i64,
}

/// List a note's captures, oldest first (capture order).
#[tauri::command]
pub fn attachments_list(
    app: AppHandle,
    state: State<AppState>,
    note_id: String,
) -> Result<Vec<Attachment>, String> {
    let recordings = app
        .path()
        .app_data_dir()
        .map(|d| sessions::recordings_dir(&d, &note_id))
        .map_err(err)?;

    let conn = state.db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, path, text, offset_ms, created_at
             FROM note_attachments WHERE note_id = ?1
             ORDER BY created_at ASC",
        )
        .map_err(err)?;
    let rows = stmt
        .query_map([&note_id], |r| {
            let kind: String = r.get(1)?;
            let rel: String = r.get(2)?;
            let path = if kind == "image" && !rel.is_empty() {
                recordings.join(&rel).to_string_lossy().to_string()
            } else {
                String::new()
            };
            Ok(Attachment {
                id: r.get(0)?,
                kind,
                path,
                text: r.get(3)?,
                offset_ms: r.get(4)?,
                created_at: r.get(5)?,
            })
        })
        .map_err(err)?;
    let mut out = Vec::new();
    for a in rows {
        out.push(a.map_err(err)?);
    }
    Ok(out)
}

/// Delete one capture by id. Removes the image file too (best-effort).
#[tauri::command]
pub fn attachment_delete(
    app: AppHandle,
    state: State<AppState>,
    note_id: String,
    id: String,
) -> Result<(), String> {
    let recordings = app
        .path()
        .app_data_dir()
        .map(|d| sessions::recordings_dir(&d, &note_id))
        .map_err(err)?;

    let conn = state.db.lock();
    // Resolve the file first so we can unlink it after the row is gone.
    let rel: Option<String> = conn
        .query_row(
            "SELECT path FROM note_attachments WHERE id = ?1 AND note_id = ?2",
            rusqlite::params![id, note_id],
            |r| r.get(0),
        )
        .ok();
    conn.execute(
        "DELETE FROM note_attachments WHERE id = ?1 AND note_id = ?2",
        rusqlite::params![id, note_id],
    )
    .map_err(err)?;
    if let Some(rel) = rel {
        if !rel.is_empty() {
            let _ = std::fs::remove_file(recordings.join(&rel));
        }
    }
    Ok(())
}

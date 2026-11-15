//! Tauri commands for silence auto-stop (silence.rs).

use crate::silence;
use crate::{db, AppState};
use serde::Serialize;
use tauri::State;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Serialize)]
pub struct SilenceStatus {
    pub enabled: bool,
    pub minutes: i64,
}

#[tauri::command]
pub fn silence_status(state: State<AppState>) -> Result<SilenceStatus, String> {
    let conn = state.db.lock();
    let cfg = silence::config(&conn);
    Ok(SilenceStatus { enabled: cfg.enabled, minutes: cfg.minutes })
}

#[tauri::command]
pub fn silence_set_enabled(state: State<AppState>, enabled: bool) -> Result<(), String> {
    let conn = state.db.lock();
    db::set_setting(&conn, silence::SETTING_ENABLED_KEY, if enabled { "true" } else { "false" })
        .map_err(err)
}

/// Set the quiet threshold in minutes (clamped 1..=120 on read).
#[tauri::command]
pub fn silence_set_minutes(state: State<AppState>, minutes: i64) -> Result<i64, String> {
    let clamped = minutes.clamp(1, 120);
    let conn = state.db.lock();
    db::set_setting(&conn, silence::SETTING_MINUTES_KEY, &clamped.to_string()).map_err(err)?;
    Ok(clamped)
}

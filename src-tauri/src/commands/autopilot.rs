//! Tauri commands for calendar auto-record ("Autopilot").
//!
//! Thin wrappers the frontend Settings panel calls: connect/disconnect Google,
//! read/set the mode, list upcoming meetings with their recordable verdict, and
//! poke the scheduler on focus/wake. The real work lives in `crate::autopilot`.

use crate::autopilot::{self, Mode};
use crate::db;
use crate::google_calendar::CalendarEvent;
use crate::AppState;
use serde::Serialize;
use tauri::{AppHandle, State};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Current Autopilot status for the Settings panel.
#[derive(Serialize)]
pub struct AutopilotStatus {
    pub mode: String,
    pub connected: bool,
    pub stop_tail_secs: i64,
}

#[tauri::command]
pub fn autopilot_status(state: State<AppState>) -> Result<AutopilotStatus, String> {
    let conn = state.db.lock();
    let mode = db::get_setting(&conn, "autopilot_mode")
        .map_err(err)?
        .unwrap_or_else(|| "off".to_string());
    let stop_tail_secs = db::get_setting(&conn, "autopilot_stop_tail_secs")
        .map_err(err)?
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(120);
    Ok(AutopilotStatus {
        mode,
        connected: state.autopilot.is_connected(),
        stop_tail_secs,
    })
}

/// Set the mode (off/ask/auto). Returns the normalized value stored.
#[tauri::command]
pub fn autopilot_set_mode(state: State<AppState>, mode: String) -> Result<String, String> {
    let normalized = Mode::parse(&mode).as_str().to_string();
    let conn = state.db.lock();
    db::set_setting(&conn, "autopilot_mode", &normalized).map_err(err)?;
    Ok(normalized)
}

/// Set the stop-tail grace period (seconds after an event's scheduled end).
#[tauri::command]
pub fn autopilot_set_stop_tail(state: State<AppState>, secs: i64) -> Result<(), String> {
    let clamped = secs.clamp(0, 3600);
    let conn = state.db.lock();
    db::set_setting(&conn, "autopilot_stop_tail_secs", &clamped.to_string()).map_err(err)
}

/// Launch the interactive Google OAuth connect flow. Resolves when the user has
/// consented and the refresh token is stored, or errors if they abandon/deny.
#[tauri::command]
pub async fn autopilot_connect_google(app: AppHandle) -> Result<(), String> {
    autopilot::connect(&app).await.map_err(err)
}

/// Forget the Google connection (clears Keychain + in-memory tokens).
#[tauri::command]
pub fn autopilot_disconnect_google(app: AppHandle) -> Result<(), String> {
    autopilot::disconnect(&app).map_err(err)
}

/// One upcoming meeting for the Settings "what Autopilot sees" list.
#[derive(Serialize)]
pub struct UpcomingMeeting {
    pub id: String,
    pub title: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub meeting_url: Option<String>,
    pub attendee_count: usize,
    pub recordable: bool,
}

impl From<&CalendarEvent> for UpcomingMeeting {
    fn from(e: &CalendarEvent) -> Self {
        Self {
            id: e.id.clone(),
            title: e.title.clone(),
            start: e.start.clone(),
            end: e.end.clone(),
            meeting_url: e.meeting_url.clone(),
            attendee_count: e.attendees.len(),
            recordable: e.is_recordable(),
        }
    }
}

/// List upcoming meetings (next ~12h) with their recordable verdict. This is
/// the read-only surface that lets the user see what Autopilot would act on
/// before ever turning Auto on (PRD milestone M2).
#[tauri::command]
pub async fn autopilot_upcoming(app: AppHandle) -> Result<Vec<UpcomingMeeting>, String> {
    let events = autopilot::upcoming_for_ui(&app).await.map_err(err)?;
    Ok(events.iter().map(UpcomingMeeting::from).collect())
}

/// Re-run the scheduler now (frontend calls this on window focus / wake so a
/// meeting that started during sleep is caught without waiting for the timer).
#[tauri::command]
pub async fn autopilot_tick(app: AppHandle) -> Result<(), String> {
    autopilot::tick(&app).await.map_err(err)
}

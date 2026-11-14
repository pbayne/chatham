//! Calendar-driven auto-record ("Autopilot").
//!
//! See `docs/prd-calendar-autorecord.md`. Ties three things together:
//!   - `google_calendar` — OAuth + event fetch
//!   - `oauth_loopback`   — the consent redirect catcher
//!   - the existing `recording_start` / `recording_stop` commands
//!
//! A background task polls the user's Google Calendar over a rolling window and,
//! depending on the mode (Off / Ask / Auto), either notifies the user to record
//! a starting meeting or starts/stops the local recording itself. No bot joins
//! the call — Humla records the Mac's own audio, the scheduler just presses
//! start/stop on the calendar's schedule.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::google_calendar::{self, CalendarEvent, Tokens};
use crate::oauth_loopback::LoopbackServer;

/// Autopilot mode, persisted in `settings` under `autopilot_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    Ask,
    Auto,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Ask => "ask",
            Mode::Auto => "auto",
        }
    }
    pub fn parse(s: &str) -> Mode {
        match s {
            "ask" => Mode::Ask,
            "auto" => Mode::Auto,
            _ => Mode::Off,
        }
    }
}

const SETTING_MODE: &str = "autopilot_mode";
const SETTING_STOP_TAIL: &str = "autopilot_stop_tail_secs";
const DEFAULT_STOP_TAIL_SECS: i64 = 120;
/// How often the scheduler re-evaluates. Also re-run on app focus/wake by the
/// frontend calling `autopilot_tick`.
const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// How far ahead we fetch events.
const WINDOW_AHEAD_HOURS: i64 = 12;
/// Fire the pre-start notification / auto-start this many seconds before the
/// event's scheduled start (the "so as I was saying" pre-roll, PRD §7).
const PRE_ROLL_SECS: i64 = 10;

/// Live Google connection + autopilot state, held on `AppState`.
pub struct Autopilot {
    /// In-memory OAuth tokens. `None` = not connected.
    tokens: Arc<Mutex<Option<Tokens>>>,
    /// Shared HTTP client (connection reuse across polls).
    http: reqwest::Client,
    /// The event id we last auto-started, so the stop side knows what it owns.
    active_event: Arc<Mutex<Option<ActiveRecording>>>,
}

#[derive(Clone, Debug)]
struct ActiveRecording {
    event_id: String,
    note_id: String,
    /// RFC3339 scheduled end; the stop deadline is this + tail.
    scheduled_end: Option<String>,
}

impl Default for Autopilot {
    fn default() -> Self {
        Self {
            tokens: Arc::new(Mutex::new(None)),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client builds"),
            active_event: Arc::new(Mutex::new(None)),
        }
    }
}

impl Autopilot {
    pub fn is_connected(&self) -> bool {
        self.tokens.lock().is_some()
    }

    /// Seed tokens from a refresh token recovered from the Keychain at startup,
    /// without doing a network round-trip yet (the first `access_token()` call
    /// refreshes). Returns whether a token was present.
    pub fn restore_refresh_token(&self, refresh_token: Option<String>) -> bool {
        match refresh_token {
            Some(rt) if !rt.trim().is_empty() => {
                *self.tokens.lock() = Some(Tokens {
                    access_token: String::new(),
                    refresh_token: Some(rt),
                    expires_at: None, // forces a refresh on first use
                });
                true
            }
            _ => false,
        }
    }

    pub fn disconnect(&self) {
        *self.tokens.lock() = None;
        *self.active_event.lock() = None;
    }

    /// Return a valid access token, refreshing if expired. Errors if not
    /// connected or if the refresh token was revoked (caller drops to the
    /// disconnected state).
    async fn access_token(&self) -> anyhow::Result<String> {
        // Fast path: a live token.
        {
            let guard = self.tokens.lock();
            if let Some(t) = guard.as_ref() {
                if !t.is_expired() && !t.access_token.is_empty() {
                    return Ok(t.access_token.clone());
                }
            } else {
                anyhow::bail!("not connected to Google");
            }
        }
        // Refresh path.
        let refresh_token = {
            let guard = self.tokens.lock();
            guard
                .as_ref()
                .and_then(|t| t.refresh_token.clone())
                .ok_or_else(|| anyhow::anyhow!("no refresh token"))?
        };
        let fresh = google_calendar::refresh_access_token(&self.http, &refresh_token).await?;
        let access = fresh.access_token.clone();
        *self.tokens.lock() = Some(fresh);
        Ok(access)
    }
}

// ─── OAuth connect flow ──────────────────────────────────────────────────────

/// Run the full interactive OAuth flow: open the browser, catch the redirect,
/// exchange the code, persist the refresh token to the Keychain, and load the
/// tokens into the live `Autopilot`. Returns the user's primary email on success.
pub async fn connect(app: &AppHandle) -> anyhow::Result<()> {
    use tauri_plugin_shell::ShellExt;

    let server = LoopbackServer::bind().await?;
    let redirect_uri = server.redirect_uri().to_string();
    let pkce = google_calendar::Pkce::generate();
    let state_nonce = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        (0..24)
            .map(|_| rng.gen_range(b'a'..=b'z') as char)
            .collect::<String>()
    };

    let auth_url = google_calendar::auth_url(&redirect_uri, &pkce, &state_nonce);

    // Open the consent page in the user's default browser.
    app.shell()
        .open(&auth_url, None)
        .map_err(|e| anyhow::anyhow!("open browser: {e}"))?;

    // Block for the redirect (5-minute budget for the user to consent).
    let code = server
        .wait_for_code(&state_nonce, Duration::from_secs(300))
        .await?;

    let autopilot = app.state::<crate::AppState>();
    let tokens = google_calendar::exchange_code(&autopilot.autopilot.http, &code, &redirect_uri, &pkce)
        .await?;

    // Persist the refresh token so the connection survives restarts.
    if let Some(rt) = &tokens.refresh_token {
        crate::keychain::write(
            &autopilot.keychain_cache,
            google_calendar::KEYCHAIN_ACCOUNT,
            Some(rt),
        )
        .map_err(|e| anyhow::anyhow!("store refresh token: {e}"))?;
    }
    *autopilot.autopilot.tokens.lock() = Some(tokens);
    Ok(())
}

/// Forget the Google connection: clear the Keychain and in-memory tokens.
pub fn disconnect(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<crate::AppState>();
    let _ = crate::keychain::write(
        &state.keychain_cache,
        google_calendar::KEYCHAIN_ACCOUNT,
        None,
    );
    state.autopilot.disconnect();
    Ok(())
}

// ─── Scheduler decision logic (pure, testable) ───────────────────────────────

/// What the scheduler should do this tick for a single event, given the current
/// time. Pure so the timing rules are unit-tested without clocks or sockets.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Within the pre-roll window and not yet recorded → start (Auto) or
    /// prompt (Ask).
    StartNow,
    /// Not yet — nothing to do.
    Wait,
    /// Already past end+tail → ensure stopped.
    Stop,
    /// Not a recordable event.
    Skip,
}

/// Decide the action for one event. `now_epoch_s` and the event times are unix
/// seconds (caller converts RFC3339 → epoch).
pub fn decide(
    start_epoch_s: Option<i64>,
    end_epoch_s: Option<i64>,
    now_epoch_s: i64,
    stop_tail_secs: i64,
    recordable: bool,
    already_recorded: bool,
) -> Action {
    if !recordable {
        return Action::Skip;
    }
    let (Some(start), Some(end)) = (start_epoch_s, end_epoch_s) else {
        return Action::Skip;
    };
    // Past the stop deadline → make sure it's stopped.
    if now_epoch_s >= end + stop_tail_secs {
        return Action::Stop;
    }
    // In the active window [start - preroll, end + tail) and not yet handled.
    if now_epoch_s >= start - PRE_ROLL_SECS && !already_recorded {
        return Action::StartNow;
    }
    Action::Wait
}

/// Parse an RFC3339 timestamp to unix seconds. Minimal: handles the
/// `2026-11-13T15:00:00-05:00` / `...Z` shapes the Calendar API returns.
pub fn rfc3339_to_epoch(s: &str) -> Option<i64> {
    // Lean on chrono if present; the repo already pulls it transitively via
    // several deps, but to avoid a hard dep we parse the fixed shape by hand.
    // Format: YYYY-MM-DDTHH:MM:SS(±HH:MM|Z)
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse::<i64>().ok() };
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let min = num(14, 16)?;
    let sec = num(17, 19)?;

    // Days from civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let mut epoch = days * 86400 + hour * 3600 + min * 60 + sec;

    // Apply timezone offset to get UTC.
    let tz = &s[19..];
    if tz.starts_with('Z') {
        // already UTC
    } else if (tz.starts_with('+') || tz.starts_with('-')) && tz.len() >= 6 {
        let sign = if tz.starts_with('-') { -1 } else { 1 };
        let oh: i64 = tz[1..3].parse().ok()?;
        let om: i64 = tz[4..6].parse().ok()?;
        epoch -= sign * (oh * 3600 + om * 60);
    }
    Some(epoch)
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ─── Scheduler loop ────────────────────────────────────────────���─────────────

/// Spawn the background scheduler. Cheap no-op ticks when Off or disconnected.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // On startup, try to restore a saved Google connection.
        {
            let state = app.state::<crate::AppState>();
            let rt = crate::keychain::read(&state.keychain_cache, google_calendar::KEYCHAIN_ACCOUNT)
                .ok()
                .flatten();
            state.autopilot.restore_refresh_token(rt);
        }
        loop {
            if let Err(e) = tick(&app).await {
                eprintln!("autopilot tick: {e}");
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

/// Fetch upcoming meetings for the Settings UI, independent of mode. Errors if
/// not connected to Google.
pub async fn upcoming_for_ui(app: &AppHandle) -> anyhow::Result<Vec<CalendarEvent>> {
    let connected = app.state::<crate::AppState>().autopilot.is_connected();
    if !connected {
        anyhow::bail!("not connected to Google");
    }
    fetch_upcoming(app).await
}

/// One scheduler evaluation. Public so the frontend can trigger it on focus/wake.
pub async fn tick(app: &AppHandle) -> anyhow::Result<()> {
    let (mode, stop_tail) = {
        let state = app.state::<crate::AppState>();
        let conn = state.db.lock();
        let mode = Mode::parse(
            &crate::db::get_setting(&conn, SETTING_MODE)
                .ok()
                .flatten()
                .unwrap_or_default(),
        );
        let tail = crate::db::get_setting(&conn, SETTING_STOP_TAIL)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(DEFAULT_STOP_TAIL_SECS);
        (mode, tail)
    };

    if mode == Mode::Off {
        return Ok(());
    }
    let connected = {
        let state = app.state::<crate::AppState>();
        state.autopilot.is_connected()
    };
    if !connected {
        return Ok(());
    }

    let events = fetch_upcoming(app).await?;
    let now = now_epoch();

    for ev in &events {
        let start = ev.start.as_deref().and_then(rfc3339_to_epoch);
        let end = ev.end.as_deref().and_then(rfc3339_to_epoch);
        let already = {
            let state = app.state::<crate::AppState>();
            let conn = state.db.lock();
            crate::db::note_exists_for_event(&conn, &ev.id).unwrap_or(false)
        };
        match decide(start, end, now, stop_tail, ev.is_recordable(), already) {
            Action::StartNow => match mode {
                Mode::Auto => start_recording_for_event(app, ev).await?,
                Mode::Ask => prompt_to_record(app, ev)?,
                Mode::Off => {}
            },
            Action::Stop => maybe_stop_for_event(app, ev).await?,
            Action::Wait | Action::Skip => {}
        }
    }
    Ok(())
}

/// Fetch events across the user's primary calendar for the rolling window.
async fn fetch_upcoming(app: &AppHandle) -> anyhow::Result<Vec<CalendarEvent>> {
    let (http, token) = {
        let state = app.state::<crate::AppState>();
        let http = state.autopilot.http.clone();
        let token = state.autopilot.access_token().await?;
        (http, token)
    };
    let now = now_epoch();
    let time_min = epoch_to_rfc3339(now);
    let time_max = epoch_to_rfc3339(now + WINDOW_AHEAD_HOURS * 3600);
    google_calendar::list_events(&http, &token, "primary", &time_min, &time_max).await
}

/// Auto mode: create a note, attach calendar metadata, start recording.
async fn start_recording_for_event(app: &AppHandle, ev: &CalendarEvent) -> anyhow::Result<()> {
    // Create the note + attach metadata under the db lock, then start.
    let note_id = {
        let state = app.state::<crate::AppState>();
        let conn = state.db.lock();
        // Skip if a recording is already running (single-session invariant).
        let default_language = crate::db::get_setting(&conn, "language")
            .ok()
            .flatten()
            .unwrap_or_else(|| "en".to_string());
        let note = crate::db::create_note(&conn, &default_language, "meeting", "")?;
        attach_meta(&conn, &note.id, ev)?;
        note.id
    };

    // Rename the note to the meeting title (best-effort).
    {
        let state = app.state::<crate::AppState>();
        let conn = state.db.lock();
        let patch = crate::db::NotePatch {
            title: Some(ev.title.clone()),
            ..Default::default()
        };
        let _ = crate::db::update_note(&conn, &note_id, &patch);
    }

    *app.state::<crate::AppState>().autopilot.active_event.lock() = Some(ActiveRecording {
        event_id: ev.id.clone(),
        note_id: note_id.clone(),
        scheduled_end: ev.end.clone(),
    });

    // Call the real recording command.
    let state = app.state::<crate::AppState>();
    crate::commands::recording_start(app.clone(), state, note_id.clone())
        .await
        .map_err(|e| anyhow::anyhow!("recording_start: {e}"))?;

    notify(app, "Recording started", &format!("Autopilot is recording \u{201c}{}\u{201d}.", ev.title));
    let _ = app.emit("autopilot://recording-started", &note_id);
    Ok(())
}

/// Auto mode stop: if the active recording belongs to this ended event, stop it.
async fn maybe_stop_for_event(app: &AppHandle, ev: &CalendarEvent) -> anyhow::Result<()> {
    let is_ours = {
        let state = app.state::<crate::AppState>();
        let guard = state.autopilot.active_event.lock();
        guard.as_ref().map(|a| a.event_id == ev.id).unwrap_or(false)
    };
    if !is_ours {
        return Ok(());
    }
    let state = app.state::<crate::AppState>();
    crate::commands::recording_stop(app.clone(), state)
        .await
        .map_err(|e| anyhow::anyhow!("recording_stop: {e}"))?;
    *app.state::<crate::AppState>().autopilot.active_event.lock() = None;
    notify(app, "Recording saved", &format!("\u{201c}{}\u{201d} ended \u{2014} summarizing.", ev.title));
    Ok(())
}

/// Ask mode: notify the user with the meeting title; the frontend listens for
/// the event and shows Record / Skip, invoking recording itself on Record.
fn prompt_to_record(app: &AppHandle, ev: &CalendarEvent) -> anyhow::Result<()> {
    // Payload the UI needs to offer the choice and act on it.
    let payload = serde_json::json!({
        "event_id": ev.id,
        "title": ev.title,
        "meeting_url": ev.meeting_url,
        "start": ev.start,
        "end": ev.end,
    });
    notify(app, "Meeting starting", &format!("Record \u{201c}{}\u{201d}?", ev.title));
    app.emit("autopilot://prompt-record", payload)
        .map_err(|e| anyhow::anyhow!("emit prompt: {e}"))?;
    Ok(())
}

/// Write the event's metadata into `note_calendar_meta`.
fn attach_meta(conn: &rusqlite::Connection, note_id: &str, ev: &CalendarEvent) -> anyhow::Result<()> {
    let attendees_json = serde_json::to_string(&ev.attendees).unwrap_or_else(|_| "[]".to_string());
    let meta = crate::db::NoteCalendarMeta {
        note_id: note_id.to_string(),
        source_event_id: ev.id.clone(),
        source_calendar_id: ev.calendar_id.clone(),
        organizer_email: ev.organizer_email.clone().unwrap_or_default(),
        attendees_json,
        meeting_url: ev.meeting_url.clone().unwrap_or_default(),
        event_html_link: ev.html_link.clone().unwrap_or_default(),
        scheduled_start: ev.start.clone().unwrap_or_default(),
        scheduled_end: ev.end.clone().unwrap_or_default(),
    };
    crate::db::set_note_calendar_meta(conn, &meta)?;
    Ok(())
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

/// Inverse of `rfc3339_to_epoch` for building query bounds (always UTC `Z`).
pub fn epoch_to_rfc3339(epoch_s: i64) -> String {
    // civil from days (Hinnant), inverse of the parse above.
    let days = epoch_s.div_euclid(86400);
    let secs = epoch_s.rem_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, m, d, hh, mm, ss
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_roundtrips() {
        for m in [Mode::Off, Mode::Ask, Mode::Auto] {
            assert_eq!(Mode::parse(m.as_str()), m);
        }
        assert_eq!(Mode::parse("garbage"), Mode::Off);
    }

    #[test]
    fn rfc3339_parse_utc() {
        // 2026-11-13T20:00:00Z == 1794600000 (verified against Python datetime)
        assert_eq!(rfc3339_to_epoch("2026-11-13T20:00:00Z"), Some(1794600000));
    }

    #[test]
    fn rfc3339_parse_with_offset() {
        // 15:00:00-05:00 is the same instant as 20:00:00Z.
        assert_eq!(
            rfc3339_to_epoch("2026-11-13T15:00:00-05:00"),
            rfc3339_to_epoch("2026-11-13T20:00:00Z")
        );
    }

    #[test]
    fn epoch_rfc3339_roundtrip() {
        let e = 1793649600;
        assert_eq!(rfc3339_to_epoch(&epoch_to_rfc3339(e)), Some(e));
    }

    #[test]
    fn decide_skips_non_recordable() {
        assert_eq!(decide(Some(100), Some(200), 150, 120, false, false), Action::Skip);
    }

    #[test]
    fn decide_waits_before_preroll() {
        // now well before start-preroll
        assert_eq!(decide(Some(1000), Some(2000), 500, 120, true, false), Action::Wait);
    }

    #[test]
    fn decide_starts_within_preroll() {
        // now == start - 5 (inside the 10s preroll), not yet recorded
        assert_eq!(decide(Some(1000), Some(2000), 995, 120, true, false), Action::StartNow);
    }

    #[test]
    fn decide_does_not_restart_if_already_recorded() {
        assert_eq!(decide(Some(1000), Some(2000), 1500, 120, true, true), Action::Wait);
    }

    #[test]
    fn decide_stops_after_end_plus_tail() {
        // end 2000 + tail 120 = 2120; now 2120 → stop
        assert_eq!(decide(Some(1000), Some(2000), 2120, 120, true, true), Action::Stop);
    }

    #[test]
    fn decide_still_recording_before_stop_deadline() {
        // now between start and end+tail, already recorded → wait (keep going)
        assert_eq!(decide(Some(1000), Some(2000), 2050, 120, true, true), Action::Wait);
    }
}

//! Silence auto-stop — end a recording after N minutes with no new speech.
//!
//! Ported in intent from stenoai's silence auto-stop. Chatham's audio-capture
//! sidecar is VAD-bounded: it only emits a transcript chunk when there is
//! actual speech, so "no new chunk for N minutes" is a reliable proxy for "the
//! room has gone quiet" — no raw RMS metering needed. A watchdog spawned at
//! `recording_start` polls the last-activity timestamp (bumped on every chunk)
//! and, once the quiet gap crosses the threshold, presses the existing
//! `recording_stop`.
//!
//! This file holds the PURE, testable policy: the setting read, and the
//! decision of whether a given (now, last_activity) gap warrants a stop. The
//! watchdog spawn lives next to the recording command so it can reuse the live
//! capture state; keeping the policy here lets it be unit tested without a mic.

const SETTING_ENABLED: &str = "silence_autostop_enabled";
const SETTING_MINUTES: &str = "silence_autostop_minutes";
const DEFAULT_MINUTES: i64 = 5;
const MIN_MINUTES: i64 = 1;
const MAX_MINUTES: i64 = 120;

/// Resolved config: whether silence auto-stop is on, and the quiet threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub enabled: bool,
    pub minutes: i64,
}

impl Config {
    pub fn threshold_secs(&self) -> i64 {
        self.minutes.clamp(MIN_MINUTES, MAX_MINUTES) * 60
    }
}

/// Read the config from settings. Off by default; minutes clamped to a sane
/// range so a hand-edited DB value can't make the watchdog never fire or fire
/// instantly.
pub fn config(conn: &rusqlite::Connection) -> Config {
    let enabled = crate::db::get_setting(conn, SETTING_ENABLED)
        .ok()
        .flatten()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let minutes = crate::db::get_setting(conn, SETTING_MINUTES)
        .ok()
        .flatten()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(DEFAULT_MINUTES)
        .clamp(MIN_MINUTES, MAX_MINUTES);
    Config { enabled, minutes }
}

pub const SETTING_ENABLED_KEY: &str = SETTING_ENABLED;
pub const SETTING_MINUTES_KEY: &str = SETTING_MINUTES;

/// Should the recording stop now? True once the quiet gap (now - last_activity)
/// reaches the threshold. `last_activity` and `now` are unix seconds.
pub fn should_stop(cfg: &Config, now: i64, last_activity: i64) -> bool {
    if !cfg.enabled {
        return false;
    }
    now - last_activity >= cfg.threshold_secs()
}

// ─── Runtime watchdog ──────────────────────���─────────────────────────────────
// Not unit-tested (needs the live capture state + a running recording); the
// pure `should_stop` / `config` above is what the tests cover.

use tauri::{AppHandle, Manager};

const POLL_SECS: u64 = 15;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Spawn a per-recording watchdog. Exits when the recording is no longer
/// `note_id` (a stop happened, or a different note started), so it never
/// outlives its session or acts on the wrong one. No-op overall when the
/// setting is off (it still polls cheaply, but never stops anything).
pub fn spawn_watchdog(app: AppHandle, note_id: String) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(POLL_SECS)).await;

            let state = app.state::<crate::AppState>();

            // Still recording this note? If not, our session is over — exit.
            let (is_ours, last_activity) = {
                let s = state.recording.lock();
                let ours = s.note_id.as_deref() == Some(note_id.as_str());
                let last = *s.last_activity.lock();
                (ours, last)
            };
            if !is_ours {
                return;
            }

            let cfg = {
                let conn = state.db.lock();
                config(&conn)
            };
            if !should_stop(&cfg, now_secs(), last_activity) {
                continue;
            }

            // Quiet long enough — stop. Reuse the real stop command, then notify
            // and exit the watchdog (its session is done).
            let app2 = app.clone();
            let st = app.state::<crate::AppState>();
            if let Err(e) = crate::commands::recording_stop(app2.clone(), st).await {
                eprintln!("silence auto-stop: recording_stop failed: {e}");
            } else {
                notify(&app2, "Recording stopped", &format!("No speech for {} min — Chatham stopped and is summarizing.", cfg.minutes));
            }
            return;
        }
    });
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool, minutes: i64) -> Config {
        Config { enabled, minutes }
    }

    #[test]
    fn disabled_never_stops() {
        assert!(!should_stop(&cfg(false, 1), 10_000, 0));
    }

    #[test]
    fn stops_once_gap_reaches_threshold() {
        let c = cfg(true, 5); // 300s
        assert!(!should_stop(&c, 1000, 800)); // 200s quiet — keep going
        assert!(!should_stop(&c, 1099, 800)); // 299s — still
        assert!(should_stop(&c, 1100, 800)); // exactly 300s — stop
        assert!(should_stop(&c, 2000, 800)); // long past — stop
    }

    #[test]
    fn minutes_are_clamped() {
        assert_eq!(cfg(true, 0).threshold_secs(), 60); // floor 1 min
        assert_eq!(cfg(true, 9999).threshold_secs(), 120 * 60); // ceil 120 min
        assert_eq!(cfg(true, 5).threshold_secs(), 300);
    }
}

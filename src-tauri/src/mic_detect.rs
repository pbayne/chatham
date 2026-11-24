//! Mic-based meeting detection ("ambient autopilot").
//!
//! A companion to calendar autopilot (`autopilot.rs`): instead of (only) the
//! calendar, a native `mic-monitor` sidecar reports which app opened the
//! microphone, and this module decides whether that app is a meeting worth
//! recording. Lets Chatham offer to record a Zoom/Teams/Meet call that was never
//! on the calendar — the highest-daily-value gap vs. stenoai.
//!
//! This file is the PURE, testable core: parsing the sidecar's JSON events,
//! matching an app against the meeting allowlist, the macOS-version gate, and
//! the debounce that stops one call from firing repeatedly. The sidecar spawn +
//! the record trigger live alongside the autopilot scheduler; keeping the
//! decision logic here means it can be unit tested without a model, a mic, or
//! the native binary.

use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// One line of the mic-monitor sidecar's stdout. The sidecar emits one JSON
/// object per line: `{"event":"start","app_id":"us.zoom.xos","app_name":"zoom.us","pid":1234,"ts":...}`.
/// On macOS 12/13 (device-level fallback) `app_id`/`app_name` are absent.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct MicEvent {
    pub event: String,
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(default)]
    pub app_name: Option<String>,
    #[serde(default)]
    pub pid: Option<i64>,
}

impl MicEvent {
    pub fn is_start(&self) -> bool {
        self.event == "start"
    }
    pub fn is_stop(&self) -> bool {
        self.event == "stop"
    }
}

/// Parse one stdout line into a `MicEvent`. Returns `None` for blank lines or
/// malformed JSON (the sidecar also logs diagnostics to stderr, and a stray
/// non-JSON line on stdout must never crash the reader).
pub fn parse_line(line: &str) -> Option<MicEvent> {
    let trimmed = line.trim();
    if trimmed.is_empty() || !trimmed.starts_with('{') {
        return None;
    }
    serde_json::from_str::<MicEvent>(trimmed).ok()
}

/// Bundle-id prefixes for apps whose mic use signals a meeting. Prefix match so
/// helper sub-processes (`com.google.Chrome.helper`, Arc's helpers) count too.
/// Ported from stenoai's `meeting-detect.js` allowlist, kept as lowercase
/// prefixes matched case-insensitively.
const MEETING_APP_PREFIXES: &[&str] = &[
    // Native videoconf / meeting apps
    "us.zoom.xos",                 // Zoom
    "com.microsoft.teams",         // Teams (classic + teams2)
    "com.cisco.webexmeetingsapp",  // Webex
    "com.webex.meetingmanager",    // Webex (alt)
    "com.apple.facetime",          // FaceTime
    "com.apple.avconferenced",     // FaceTime / Phone.app / Continuity call audio
    "com.hnc.discord",             // Discord
    "com.tinyspeck.slackmacgap",   // Slack huddles
    "com.logmein.gotomeeting",     // GoToMeeting
    "com.bluejeansnet.bluejeans",  // BlueJeans
    "co.pop.desktop",              // Pop
    "com.google.meetings",         // Google Meet (standalone PWA)
    "com.apple.voicememos",        // Voice Memos
    // Browsers — most web meetings route mic capture through here
    "com.apple.webkit",            // Safari (helper)
    "com.apple.safari",            // Safari (main)
    "com.google.chrome",           // Chrome (+ helpers)
    "org.chromium.",               // Chromium
    "com.microsoft.edgemac",       // Edge
    "company.thebrowser.",         // Arc (app + helpers)
    "com.brave.browser",           // Brave
    "org.mozilla.",                // Firefox
];

/// Whether an app's bundle id marks it as a meeting app. `allow_device_level`
/// decides the macOS 12/13 case where the sidecar can't attribute an app:
/// those older systems emit app-less device-level signals that the legacy path
/// still treats as a meeting; macOS 14+ always attributes, so an app-less event
/// there is an AEC / notification-ping artifact and is NOT a meeting.
pub fn is_meeting_app(event: &MicEvent, allow_device_level: bool) -> bool {
    match event.app_id.as_deref() {
        None => allow_device_level,
        Some(id) => {
            let id = id.to_ascii_lowercase();
            MEETING_APP_PREFIXES.iter().any(|p| id.starts_with(p))
        }
    }
}

/// Whether mic auto-detect may run on this OS version. The reliable per-app
/// signal (`kAudioProcessPropertyIsRunningInput` over the process-object list)
/// only exists on macOS 14+. Below that the sidecar falls back to a coarse
/// device-level signal that misfires, so the whole feature is gated to 14+.
///
/// Parse direction on failure is PERMISSIVE: an unparseable version returns
/// true, because the feature is opt-in and the sidecar itself guards the old
/// device-level path. `major` is the major component of the OS version string.
pub fn os_supported(major: Option<u32>) -> bool {
    match major {
        Some(m) => m >= 14,
        None => true, // permissive on parse failure — see doc comment
    }
}

/// Parse a macOS version string ("14.5.1", "15") into its major component.
pub fn parse_major(version: &str) -> Option<u32> {
    version.split('.').next()?.trim().parse::<u32>().ok()
}

/// Debounce for mic-start events. One meeting opens and closes the mic many
/// times (mute/unmute, device switches, browser tab churn), and a call can run
/// for an hour. Without a cooldown the "record?" prompt would fire repeatedly.
/// This tracks the last time we acted and refuses a new trigger inside the
/// cooldown window.
#[derive(Debug)]
pub struct Debouncer {
    cooldown_secs: i64,
    last_fired_at: Option<i64>,
}

impl Debouncer {
    pub fn new(cooldown_secs: i64) -> Self {
        Self { cooldown_secs, last_fired_at: None }
    }

    /// Should a trigger at `now` (unix secs) be allowed through? Updates the
    /// last-fired timestamp when it returns true.
    pub fn allow(&mut self, now: i64) -> bool {
        let ok = match self.last_fired_at {
            None => true,
            Some(last) => now - last >= self.cooldown_secs,
        };
        if ok {
            self.last_fired_at = Some(now);
        }
        ok
    }

    /// Clear the cooldown (e.g. when recording stops, so the next genuine
    /// meeting can trigger immediately).
    pub fn reset(&mut self) {
        self.last_fired_at = None;
    }
}

/// Guards the mic-RELEASE auto-stop against false positives. A meeting app
/// releasing the mic is not reliably "call ended" — muting, a device switch, a
/// notification ping, or a WebRTC renegotiation can release then re-acquire the
/// mic within seconds. So a release ARMS a pending stop tagged with the current
/// "start generation"; the stop only fires if, when its grace window elapses,
/// no meeting-app START has bumped the generation since. A reopened mic bumps
/// the generation and abandons the pending stop. This coalesces the
/// release+reacquire flap that mute does, and only a genuine end (mic stays
/// released past the window) actually stops.
#[derive(Debug, Default)]
pub struct StopGrace {
    /// Bumped on every meeting-app start; a pending stop captures this value.
    generation: u64,
}

impl StopGrace {
    pub fn new() -> Self {
        Self { generation: 0 }
    }

    /// A meeting app opened the mic — invalidate any pending stop.
    pub fn note_start(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// A meeting app released the mic — arm a stop and return the generation
    /// token to re-check after the grace window.
    pub fn arm(&self) -> u64 {
        self.generation
    }

    /// At fire time: should the armed stop (holding `token`) proceed? Only if no
    /// start has bumped the generation since it was armed.
    pub fn should_fire(&self, token: u64) -> bool {
        self.generation == token
    }
}


// ─── Runtime: sidecar watcher ────────────────────────────────────────────────
// Everything below drives the native mic-monitor sidecar and ties a detected
// meeting to the autopilot record path. It is NOT unit-tested here (needs the
// native binary + a real mic + the Tauri app); the testable decision core above
// is what the tests exercise. Kept in this module so the policy and the wiring
// that applies it live together.

const SETTING_MIC_DETECT: &str = "mic_detect_enabled";
/// Cooldown so one call's mute/unmute churn can't re-prompt. 5 minutes.
const MIC_DETECT_COOLDOWN_SECS: i64 = 300;
/// Grace window after a meeting app releases the mic before the recording is
/// auto-stopped. Absorbs mute/device-switch/notification flaps that release and
/// re-acquire the mic within a few seconds; only a mic that STAYS released past
/// this window ends the recording.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(12);

/// Whether mic-based detection is switched on (persisted). Off by default —
/// like stenoai, this is opt-in.
pub fn is_enabled(conn: &rusqlite::Connection) -> bool {
    crate::db::get_setting(conn, SETTING_MIC_DETECT)
        .ok()
        .flatten()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

/// Shared watcher handle on `AppState`, so the toggle can start it and a second
/// start is a no-op.
#[derive(Default)]
pub struct MicWatcher {
    running: Arc<Mutex<bool>>,
}

impl MicWatcher {
    fn try_claim(&self) -> bool {
        let mut g = self.running.lock();
        if *g {
            return false;
        }
        *g = true;
        true
    }
    fn release(&self) {
        *self.running.lock() = false;
    }
}

/// Resolve the mic-monitor sidecar: next to the executable in production, under
/// `src-tauri/binaries/` in dev. Mirrors `diarize::sidecar_path`.
fn sidecar_path() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in [
                "mic-monitor",
                "mic-monitor-aarch64-apple-darwin",
                "mic-monitor-x86_64-apple-darwin",
            ] {
                let p = dir.join(name);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for triple in ["aarch64-apple-darwin", "x86_64-apple-darwin"] {
            for base in ["src-tauri/binaries", "binaries"] {
                let p = cwd.join(format!("{base}/mic-monitor-{triple}"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Start the mic-monitor watcher if the OS supports it and it isn't already
/// running. Spawns the sidecar, reads its JSON line stream, and triggers the
/// autopilot record path on a detected meeting. Idempotent.
pub fn start_watcher(app: AppHandle) {
    let major = parse_major(&os_version_string());
    if !os_supported(major) {
        eprintln!("mic-detect: OS unsupported (needs macOS 14+), not starting");
        return;
    }
    let state = app.state::<crate::AppState>();
    if !state.mic_watcher.try_claim() {
        return;
    }
    let allow_device_level = matches!(major, Some(m) if m < 14);

    // `tauri::async_runtime::spawn`, NOT bare `tokio::spawn`: `start_watcher`
    // is called from `setup()` on the main thread (and again from the Settings
    // toggle), where no Tokio runtime is entered. Bare `tokio::spawn` there
    // panics ("there is no reactor running") and, because the call can originate
    // inside tao's Objective-C launch callback, the panic cannot unwind and
    // aborts the whole app on launch (#crash-triage). The Tauri runtime handle
    // is always valid here.
    tauri::async_runtime::spawn(async move {
        run_watch_loop(app.clone(), allow_device_level).await;
        app.state::<crate::AppState>().mic_watcher.release();
    });
}

fn os_version_string() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Whether this running OS supports mic-based detection (macOS 14+). Used by
/// the Settings command to disable the toggle with an explanation.
pub fn os_supported_here() -> bool {
    os_supported(parse_major(&os_version_string()))
}

async fn run_watch_loop(app: AppHandle, allow_device_level: bool) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command;

    let Some(path) = sidecar_path() else {
        eprintln!("mic-detect: mic-monitor sidecar not found; watcher idle");
        return;
    };

    let mut child = match Command::new(&path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mic-detect: spawn mic-monitor: {e}");
            return;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return;
    };

    let mut debounce = Debouncer::new(MIC_DETECT_COOLDOWN_SECS);
    let grace = std::sync::Arc::new(parking_lot::Mutex::new(StopGrace::new()));
    let mut reader = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        // Honor the live setting each event — the user may toggle it off.
        let enabled = {
            let state = app.state::<crate::AppState>();
            let conn = state.db.lock();
            is_enabled(&conn)
        };
        if !enabled {
            break; // turned off: exit; re-enabling restarts the watcher
        }
        let Some(event) = parse_line(&line) else {
            continue;
        };
        if !is_meeting_app(&event, allow_device_level) {
            continue;
        }
        if event.is_start() {
            // A (re)opened mic invalidates any pending auto-stop, then may
            // trigger an offer/start of its own (subject to the debounce).
            grace.lock().note_start();
            if debounce.allow(now_secs()) {
                trigger_for_event(&app, &event).await;
            }
        } else if event.is_stop() {
            // The call's mic was released. Arm a grace-delayed stop: fire only
            // if no meeting-app start reopens the mic within the window (mute
            // flaps, device switches, notification pings release+reacquire).
            let token = grace.lock().arm();
            let app_for_stop = app.clone();
            let grace_for_stop = grace.clone();
            tokio::spawn(async move {
                tokio::time::sleep(STOP_GRACE).await;
                if !grace_for_stop.lock().should_fire(token) {
                    return; // mic reopened within the window — not a real end
                }
                if let Err(e) = crate::autopilot::stop_ambient_recording(&app_for_stop).await {
                    eprintln!("mic-detect: auto-stop failed: {e:#}");
                }
            });
        }
    }

    let _ = child.kill().await;
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A meeting app opened the mic. In Auto mode start recording now; in Ask mode
/// prompt the frontend; in Off do nothing. Reuses the autopilot mode so mic +
/// calendar detection share one user preference.
async fn trigger_for_event(app: &AppHandle, event: &MicEvent) {
    let mode = {
        let state = app.state::<crate::AppState>();
        let conn = state.db.lock();
        crate::autopilot::Mode::parse(
            &crate::db::get_setting(&conn, "autopilot_mode")
                .ok()
                .flatten()
                .unwrap_or_default(),
        )
    };
    let app_name = event
        .app_name
        .clone()
        .or_else(|| event.app_id.clone())
        .unwrap_or_else(|| "A meeting app".to_string());

    match mode {
        crate::autopilot::Mode::Off => {}
        crate::autopilot::Mode::Ask => {
            notify(app, "Meeting detected", &format!("{app_name} opened the mic — record?"));
            let payload = serde_json::json!({
                "source": "mic",
                "app_id": event.app_id,
                "app_name": event.app_name,
            });
            let _ = app.emit("autopilot://prompt-record", payload);
        }
        crate::autopilot::Mode::Auto => {
            if let Err(e) = crate::autopilot::start_ambient_recording(app, &app_name).await {
                eprintln!("mic-detect: auto-start failed: {e}");
            }
        }
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_start_event() {
        let e = parse_line(r#"{"event":"start","app_id":"us.zoom.xos","app_name":"zoom.us","pid":1234,"ts":1.5}"#).unwrap();
        assert!(e.is_start());
        assert_eq!(e.app_id.as_deref(), Some("us.zoom.xos"));
    }

    #[test]
    fn parses_an_app_less_event() {
        let e = parse_line(r#"{"event":"start"}"#).unwrap();
        assert!(e.is_start());
        assert_eq!(e.app_id, None);
    }

    #[test]
    fn ignores_blank_and_nonjson_lines() {
        assert!(parse_line("").is_none());
        assert!(parse_line("   ").is_none());
        assert!(parse_line("mic-monitor: starting up").is_none());
    }

    #[test]
    fn matches_meeting_apps_by_prefix() {
        let zoom = MicEvent { event: "start".into(), app_id: Some("us.zoom.xos".into()), app_name: None, pid: None };
        assert!(is_meeting_app(&zoom, false));

        // Chrome helper sub-process still matches via prefix.
        let chrome_helper = MicEvent { event: "start".into(), app_id: Some("com.google.Chrome.helper".into()), app_name: None, pid: None };
        assert!(is_meeting_app(&chrome_helper, false));

        // Case-insensitive.
        let teams = MicEvent { event: "start".into(), app_id: Some("COM.MICROSOFT.TEAMS2".into()), app_name: None, pid: None };
        assert!(is_meeting_app(&teams, false));
    }

    #[test]
    fn rejects_non_meeting_apps() {
        let music = MicEvent { event: "start".into(), app_id: Some("com.apple.Music".into()), app_name: None, pid: None };
        assert!(!is_meeting_app(&music, false));
        assert!(!is_meeting_app(&music, true)); // device-level fallback doesn't rescue a known non-meeting app
    }

    #[test]
    fn app_less_event_follows_device_level_flag() {
        let bare = MicEvent { event: "start".into(), app_id: None, app_name: None, pid: None };
        assert!(!is_meeting_app(&bare, false)); // macOS 14+: app-less = artifact
        assert!(is_meeting_app(&bare, true)); // macOS 12/13: device-level counts
    }

    #[test]
    fn os_gate() {
        assert!(!os_supported(Some(13)));
        assert!(os_supported(Some(14)));
        assert!(os_supported(Some(15)));
        assert!(os_supported(None)); // permissive on parse failure
    }

    #[test]
    fn parses_major_version() {
        assert_eq!(parse_major("14.5.1"), Some(14));
        assert_eq!(parse_major("15"), Some(15));
        assert_eq!(parse_major("nonsense"), None);
    }

    #[test]
    fn debounce_blocks_within_cooldown() {
        let mut d = Debouncer::new(300);
        assert!(d.allow(1000)); // first fires
        assert!(!d.allow(1100)); // 100s later — blocked
        assert!(!d.allow(1299)); // 299s — still blocked
        assert!(d.allow(1300)); // exactly cooldown — allowed
    }

    #[test]
    fn debounce_reset_allows_immediately() {
        let mut d = Debouncer::new(300);
        assert!(d.allow(1000));
        d.reset();
        assert!(d.allow(1001)); // reset clears the cooldown
    }

    #[test]
    fn stop_grace_fires_when_mic_stays_released() {
        let g = StopGrace::new();
        let token = g.arm(); // mic released → arm a stop
        // no start in the window → the armed stop should fire
        assert!(g.should_fire(token));
    }

    #[test]
    fn stop_grace_cancelled_by_a_reopen() {
        let mut g = StopGrace::new();
        let token = g.arm(); // mic released → arm
        g.note_start(); // mic reopened within the window (mute flap)
        assert!(!g.should_fire(token), "a reopen must abandon the pending stop");
    }

    #[test]
    fn stop_grace_latest_release_wins_after_a_flap() {
        let mut g = StopGrace::new();
        let stale = g.arm(); // first release
        g.note_start(); // reopened — stale token now invalid
        let fresh = g.arm(); // released again
        assert!(!g.should_fire(stale), "the pre-flap stop stays cancelled");
        assert!(g.should_fire(fresh), "the post-flap release can still end it");
    }

    #[test]
    fn stop_grace_independent_tokens_across_many_starts() {
        let mut g = StopGrace::new();
        for _ in 0..5 {
            g.note_start();
        }
        let token = g.arm();
        assert!(g.should_fire(token));
        g.note_start();
        assert!(!g.should_fire(token));
    }
}

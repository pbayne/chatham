//! Mic-based meeting detection ("ambient autopilot").
//!
//! A companion to calendar autopilot (`autopilot.rs`): instead of (only) the
//! calendar, a native `mic-monitor` sidecar reports which app opened the
//! microphone, and this module decides whether that app is a meeting worth
//! recording. Lets Humla offer to record a Zoom/Teams/Meet call that was never
//! on the calendar — the highest-daily-value gap vs. stenoai.
//!
//! This file is the PURE, testable core: parsing the sidecar's JSON events,
//! matching an app against the meeting allowlist, the macOS-version gate, and
//! the debounce that stops one call from firing repeatedly. The sidecar spawn +
//! the record trigger live alongside the autopilot scheduler; keeping the
//! decision logic here means it can be unit tested without a model, a mic, or
//! the native binary.

use serde::Deserialize;

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
}

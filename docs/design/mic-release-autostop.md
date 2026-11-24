# Design: stop recording when the call's mic is released (mic-detect auto-stop)

Status: DESIGN (pre-implementation). Branch `spike/turso-local-store`.
Goal: when a mic-detected meeting ends (the meeting app releases the mic),
stop the recording that mic-detection started — the symmetric half of the
already-built "meeting app opened the mic → offer/start recording."

## What exists today (don't rebuild)
- `mic_detect.rs` runs the `mic-monitor` sidecar, which emits one JSON line per
  mic event: `{"event":"start"|"stop","app_id":...,"app_name":...,"pid":...}`.
- `MicEvent::is_start()` / `is_stop()` both parse. The watch loop
  (`run_watch_loop`) currently forwards ONLY `is_start()` (line ~313) — stop
  events are parsed and dropped.
- `start_ambient_recording` (autopilot.rs) creates a note, registers
  `active_event = ActiveRecording { event_id: "mic:<note_id>", note_id,
  scheduled_end: None }`, and calls `recording_start`.
- `maybe_stop_for_event` (autopilot.rs) already stops the active recording when
  it belongs to a given event id, then clears `active_event`. The CALENDAR path
  uses it; the mic path has no equivalent trigger.

## The gap
A mic-started recording has `scheduled_end: None` (no calendar time), so the
calendar stop never fires. Nothing watches the mic RELEASE. So a mic-detected
call records until the user stops it by hand.

## Design

### 1. Forward stop events from the watch loop
In `run_watch_loop`, after the existing start handling, also handle stop:
- keep the `enabled` check and `is_meeting_app` gate (same as start — a stop
  from a non-meeting app is irrelevant);
- on a meeting-app stop event, call a new `trigger_stop_for_event(app, &event)`.
- Do NOT run stop through the start `Debouncer` (that one suppresses rapid
  re-starts); the grace logic below is the stop's own guard.

### 2. Grace period — the false-positive guard (REQUIRED)
A mic release is NOT reliably "call ended": muting, a device switch, a
notification ping, or a brief WebRTC renegotiation can release+reacquire the
mic within seconds. Stopping on the first release would cut recordings mid-call.

Rule: on a meeting-app stop event, arm a grace timer (default 12s — tunable).
When it elapses, stop ONLY if the mic has NOT been reopened by a meeting app in
the interval. Implementation: a monotonically increasing "mic generation"
counter bumped on every meeting-app START; capture it when arming, and at
fire-time stop only if the counter is unchanged AND no start arrived. A reopened
mic bumps the counter → the pending stop is abandoned. (This also naturally
coalesces the release+reacquire that Zoom does on mute.)

### 3. stop_ambient_recording(app, )  [new, autopilot.rs]
Mirror of `maybe_stop_for_event`, gated to mic-started sessions:
- read `active_event`; proceed only if `event_id.starts_with("mic:")` (never
  auto-stop a recording the USER started manually or a calendar recording —
  those aren't ours to end on a mic blip);
- call `recording_stop`, clear `active_event`, notify "Recording saved — the
  meeting ended."
- No-op (Ok) if there's no active mic recording.

### 4. Mode gating
Honor `autopilot_mode` the same way start does:
- Auto: stop automatically after the grace period.
- Ask: this is the subtle one. For v1, auto-stop fires in BOTH Ask and Auto —
  rationale: the user already consented to THIS recording (Ask prompted them to
  start it, or Auto started it), so ending it when the call ends is not a new
  decision, it's completing the one they made. (If this feels too aggressive,
  a later refinement can prompt "Call ended — stop recording?" in Ask mode.)
- Off: the watcher does nothing anyway.

## Safety / invariants
- Only stops `mic:`-prefixed active events → never touches manual or calendar
  recordings.
- `recording_stop` is the SAME command the user's Stop button calls, so the
  existing post-stop chain, slot lifecycle, and the (separately-tracked)
  "record immediately after" decoupling all apply unchanged. This feature adds
  a TRIGGER for stop; it does not change what stop does.
- Grace timer is best-effort: if the app quits during the window, nothing stops
  — identical to today (user stops manually). No new persistence needed.
- Debounce vs grace are independent: start debounce prevents offer-spam; stop
  grace prevents premature cutoff. Don't share state.

## Tests
- `is_stop()` for a meeting app arms a stop; a start before the grace elapses
  cancels it (generation counter bumped).
- A stop for a non-meeting app is ignored.
- `stop_ambient_recording` no-ops when `active_event` is None or non-`mic:`.
- `stop_ambient_recording` stops + clears when `active_event` is `mic:`.
- Grace: two stops in a row (mute flap) don't double-stop; reopen cancels.

## Out of scope (tracked elsewhere)
- "Record immediately after stopping" (post-stop chain off-slot decoupling) —
  separate design, separate change.
- The "already-open mic at launch emits no start event" miss — a separate
  detection-coverage gap (would need the sidecar to emit current state on
  startup); note it, don't fold it in here.

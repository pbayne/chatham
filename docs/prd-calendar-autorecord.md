# PRD — Calendar-driven auto-record ("Autopilot")

**Status:** Draft
**Author:** Patrick (with Claude)
**Date:** 2026-11-13
**Closes the gap from:** `docs/competitor-steno-teardown.md` §2 and §5 item 10 — *"Auto start/stop meeting detection… the highest-value feature Steno has that Humla lacks, and the one users notice daily."*

---

## 0. The one-paragraph version

Humla reads the user's macOS calendar on-device (EventKit), notices when a meeting
with an audio component is about to start, and offers to record it — one click, or
fully automatic. When the meeting ends it stops on its own, so a finished,
summarized note is waiting without the user ever reaching for the app. No bot joins
the call; Humla keeps doing what it already does — capture the Mac's own mic + system
audio — it just *starts and stops itself on the calendar's schedule*.

This is explicitly **not** a dial-in meeting bot (Recall.ai / Otter model). Humla is a
local system-audio recorder. "Auto-join" here means "auto-start the local recording
when your calendar says a meeting is happening," optionally opening the meeting link.
That distinction is the whole design and it keeps Humla's local-first, bot-free,
BYOK values intact.

---

## 1. Why this, why now

- Your own teardown ranks it the single most valuable feature Steno has and Humla
  lacks, and the one that most often swings a side-by-side trial.
- It compounds Humla's existing strengths rather than chasing Steno's chat polish:
  the thing that gets recorded is still diarized, cited, multilingual — the three
  defensible wins from the teardown's closing note.
- The plumbing it needs already exists: `recording_start(note_id)` /
  `recording_stop`, `ipc.createNote()`, and the `notification` + `autostart` Tauri
  plugins are all in the tree today. The new surface area is a calendar reader and a
  scheduler — not a rewrite.

## 2. Non-goals

- **No dial-in bot.** Humla never appears as a participant in Zoom/Meet/Teams. If no
  audio plays on the Mac, nothing useful is captured — that's acceptable and must be
  communicated, not hidden.
- **No cloud calendar APIs.** We read the local Calendar.app store via EventKit,
  which already mirrors the user's Google/Microsoft/iCloud accounts. No OAuth, no
  Google Calendar API, no new secrets. Keeps the privacy story clean.
- **No Windows.** Consistent with the existing macOS-only sidecar commitment.
- **Not** a rewrite of the recording pipeline. Autopilot calls the same commands a
  human click does.

## 3. User-facing behavior

### 3.1 Modes (single setting, three values)
- **Off** (default) — today's behavior, fully manual.
- **Ask** — a notification fires ~30 s before a detected meeting: *"Record 'Weekly
  Sync'? [Record] [Skip]"*. Recording starts only on Record.
- **Auto** — recording starts automatically at meeting start, with a notification
  that it began and a one-click **Stop** / **Don't record this one**.

### 3.2 What counts as a recordable meeting (heuristics)
An EventKit event qualifies when, at its start time, it:
1. is not all-day, not declined, not marked free/transparent;
2. has either a known meeting URL (Zoom/Meet/Teams/Webex regex over
   `location` + `notes` + `url`) **or** ≥ 2 attendees;
3. has non-zero duration and hasn't already ended.

A per-calendar allowlist lets the user pick *which* calendars Autopilot watches
(e.g. work only, not personal). Default: all writable calendars with attendees.

### 3.3 Start / stop semantics
- **Start:** create a note titled from the event (`event.title`), stamp its scheduled
  time, optionally open the meeting URL (`Ask`/`Auto` setting "open link on start"),
  then call `recording_start(note_id)`. The note carries the event's attendee list
  into the custom-vocabulary trail so names transcribe correctly — a quiet quality win.
- **Stop:** at `event.end` (plus a configurable tail, default 2 min, to catch
  overruns), fire `recording_stop`. Manual stop always wins. If the user is mid-meeting
  past the scheduled end, a "still going — keep recording?" nudge defers the auto-stop
  once.
- **Overlap / conflicts:** Humla records one meeting at a time (hard backend
  invariant, see `recording_start`'s stop-in-progress guard). If two events overlap,
  Autopilot records the first that started and notifies that the second was skipped.
- **Silence guard:** if `Auto` starts a recording and the mic + system streams stay
  below the RMS gate for N minutes (reuse `wav::rms`), surface *"No audio detected —
  still recording?"* so an auto-start on a meeting that never actually happened doesn't
  silently burn disk.

### 3.4 First-run & permissions
- Calendar is a new TCC permission. Request it lazily — only when the user first
  switches Autopilot off Off — with a plain-language explainer: *"Humla reads your
  calendar on this Mac to know when meetings start. It never uploads your calendar."*
- Pair with the existing autostart plugin: Autopilot is only useful if Humla is
  running, so switching to Ask/Auto offers to enable launch-at-login (`--hidden`,
  which the LaunchAgent already supports).

## 4. Architecture

```
┌───────────────────────────────────────────────┐
│ React (src/)                                   │
│  Settings → Autopilot (Off/Ask/Auto, calendars,│
│  tail, open-link)  ·  menubar status  ·        │
│  record/skip notifications                     │
└───────────────┬───────────────────────────────┘
                │ Tauri IPC (invoke / events)
┌───────────────▼───────────────────────────────┐
│ Rust (src-tauri/src/autopilot.rs)  [NEW]       │
│  • scheduler: tokio task, wakes on the next    │
│    event boundary, not a busy poll             │
│  • heuristics: is_recordable(event)            │
│  • drives existing recording_start/_stop       │
│  • settings keys in the settings table         │
│  • emits events the UI/menubar subscribe to    │
└───────────────┬───────────────────────────────┘
                │ spawns / line-protocol stdout (sidecar pattern,
                │ mirrors audio-capture & speaker-diarize)
┌───────────────▼───────────────────────────────┐
│ calendar-watch (Swift sidecar)  [NEW]          │
│  EventKit: request access, enumerate events in │
│  a rolling window, emit JSON on change:        │
│  {event:"meeting", id, title, start_ms,        │
│   end_ms, url?, attendees:[…], calendar}       │
│  Re-queries on EKEventStoreChanged + on a      │
│  cheap timer. No network.                      │
└────────────────────────────────────────────────┘
```

Why a Swift sidecar rather than an `objc2-event-kit` crate: EventKit's async
authorization + `EKEventStoreChanged` notifications are far less friction in Swift,
and the project already has two Swift sidecars with a build-script + line-protocol
pattern to copy (`scripts/build-sidecar.sh`). Consistency beats shaving one process.

### New settings keys (settings table, same pattern as `transcribe_config`)
- `autopilot_mode` — `off` | `ask` | `auto` (default `off`)
- `autopilot_calendars` — JSON array of EKCalendar identifiers to watch (empty = all)
- `autopilot_open_link` — bool, open meeting URL on start (default false)
- `autopilot_stop_tail_secs` — int, grace period after scheduled end (default 120)

## 5. Milestones (each independently shippable + testable)

1. **M1 — calendar-watch sidecar.** Swift package, EventKit access prompt, emits the
   rolling event window as JSON. Verify by running the binary standalone and seeing
   real calendar events print. Build script + `tauri.conf.json` externalBin wiring.
2. **M2 — read-only surfacing.** `autopilot.rs` ingests sidecar events; a new
   Settings → Autopilot panel lists today's detected meetings with the
   recordable/not-recordable verdict and reason. No recording yet. This proves the
   heuristics against the user's real calendar before anything auto-fires.
3. **M3 — Ask mode.** Notification 30 s pre-start, Record/Skip, creates note + calls
   `recording_start`; auto-stop at end+tail. Manual stop wins. The core loop.
4. **M4 — Auto mode + guards.** Auto-start, silence guard, overlap handling,
   still-going nudge, launch-at-login offer.
5. **M5 — polish.** Attendee names into the vocab trail, menubar "next: ⏺ 2:00 PM —
   Weekly Sync", per-calendar allowlist UI, docs page.

## 6. Risks & mitigations

- **TCC calendar denial.** Degrade to Off with a clear "calendar access needed" state
  and a button that deep-links to System Settings. Never nag.
- **Recording silence (no audio played).** §3.3 silence guard; plus the Ask default
  for new users so nothing auto-fires until they opt into Auto.
- **Clock/sleep drift.** macOS sleep can swallow a scheduled wake. Scheduler must
  re-evaluate on wake (`NSWorkspace` did-wake, surfaced through the sidecar or a Rust
  wake hook) and start a meeting already in progress, not just future ones.
- **Privacy optics.** Reading a calendar *feels* invasive even though it's local. The
  explainer copy and the "never uploads your calendar" line are load-bearing; put it
  in the permission prompt and the docs page, not buried.
- **Double-record with manual use.** The backend stop-in-progress / single-session
  invariant already prevents two concurrent recordings; Autopilot must surface *why*
  it skipped rather than failing silently.

## 6a. Calendar source: Google Calendar API (chosen) vs EventKit

**Decision: Google Calendar API directly (OAuth), not EventKit.**

Reality check that drove this: the target users (Patrick + colleague) run Google
Calendar and do **not** mirror it into macOS Internet Accounts — `~/Library/Calendars`
is absent and Calendar.app has no accounts. An EventKit sidecar would read an empty
store. Rather than require every user to add a Google account in System Settings
(a manual step that fights the "as easy as possible" goal, and may be MDM-blocked on
corp machines), Humla talks to Google Calendar directly.

This **overrides the §2 non-goal** "no cloud calendar APIs / no OAuth." The reason is
not just availability — it's the metadata. The Google Calendar API returns richer,
cleaner event data than EventKit surfaces, and that metadata is the point: it gets
attached to each recording's note.

### What we pull per event (and attach to the note)
- `summary` (title), `description`, `location`
- `start` / `end` (+ timezone), recurrence (`recurringEventId`, `recurrence`)
- `organizer` (name + email), `attendees[]` with **`responseStatus`**
  (accepted/declined/tentative) and `self`/`organizer` flags
- `conferenceData` — structured Meet/Zoom entry points (`entryPoints[].uri`,
  `conferenceSolution.name`), far more reliable than regexing a URL out of notes
- `hangoutLink`, `htmlLink` (deep-link back to the event), `status`, `visibility`
- `attachments[]` (Drive agenda docs), `colorId`

### How it maps into Humla
- **Note title** ← event `summary`.
- **Attendee names** ← `attendees[].displayName` → fed into the Whisper
  `initial_prompt` vocab trail, so names transcribe correctly (the M5 quality win,
  now sourced from real metadata instead of EventKit's thinner attendee list).
- **New note metadata columns** (migration on `notes`, same style as
  `summary_provider`): `source_event_id`, `source_calendar_id`, `organizer_email`,
  `attendees_json`, `meeting_url`, `event_html_link`, `scheduled_start`, `scheduled_end`.
  These make a recording traceable back to its calendar event and let future features
  (search by attendee, "meetings with X", agenda-aware summaries) build on stored
  facts rather than re-fetching.
- **Recordable heuristic** gets better: use `responseStatus != "declined"` and
  `conferenceData` presence directly, instead of inferring from attendee count.

### OAuth design (reuses existing infra)
- **Flow:** OAuth 2.0 Authorization Code **+ PKCE**, loopback redirect
  (`http://127.0.0.1:<ephemeral>`), the installed-app pattern. No client secret ships
  in the binary (PKCE replaces it); a public OAuth **client ID** is embedded, which is
  standard and safe for installed apps.
- **Scope:** `https://www.googleapis.com/auth/calendar.events.readonly` — read-only,
  events only. Narrowest scope that works. Surface it verbatim in the consent explainer.
- **Browser:** open the consent URL with `tauri-plugin-shell` opener (already wired);
  spin a one-shot loopback listener (tiny `tokio` TCP accept, or `tiny_http`) to catch
  the redirect with the auth code, then shut it down.
- **Token storage:** refresh token → **Keychain** via the existing `keychain` module
  (same mechanism as provider API keys in `commands/api_keys.rs`). Access token kept
  in memory on `AppState`, refreshed on 401. Nothing calendar-related written to
  plaintext.
- **HTTP:** `reqwest` (already a dep, rustls) against
  `GET /calendar/v3/users/me/calendarList` (pick calendars) and
  `GET /calendar/v3/calendars/{id}/events?timeMin=…&timeMax=…&singleEvents=true&orderBy=startTime`
  for the rolling window. `singleEvents=true` expands recurrences so the scheduler
  sees concrete instances.
- **Polling vs push:** v1 polls the events list every ~2–5 min over a rolling window
  (next ~12 h) and on app focus/wake. Google *push notifications* (watch channels)
  need a public HTTPS webhook — not available for a local app, so explicitly out.
  Incremental `syncToken` keeps polls cheap after the first fetch.

### What this changes in the architecture (§4)
The `calendar-watch` **Swift/EventKit sidecar is dropped**. Its role moves into
`autopilot.rs` (Rust) + a small `google_calendar.rs` module:
```
┌───────────────────────────────────────────────┐
│ Rust (src-tauri/src/)                          │
│  autopilot.rs   — scheduler + heuristics       │
│  google_calendar.rs [NEW] — OAuth(PKCE),       │
│    token refresh (Keychain), events() fetch,   │
│    syncToken, calendarList                     │
│    → reqwest, no sidecar, no EventKit          │
└───────────────────────────────────────────────┘
```
Fewer moving parts than the sidecar design (no second Swift package, no TCC calendar
prompt), at the cost of an OAuth consent flow and a Google Cloud OAuth client ID that
has to exist once.

### New setup cost (one-time, by you not the colleague)
- Create a Google Cloud project, enable the Calendar API, configure an OAuth consent
  screen (External, in "Testing" is fine for a handful of users; or publish), and make
  an **OAuth client ID of type "Desktop app."** Embed that client ID in the build.
- While the consent screen is in "Testing," each user's Google account must be added
  as a **test user** (up to 100) — otherwise Google blocks consent. This is the one
  gating admin step; note it in README so the colleague isn't surprised by an
  "unverified app" screen. Publishing the consent screen removes the test-user list
  but triggers Google's verification for sensitive scopes.

### Risks specific to Path B
- **"Unverified app" scare screen** in Testing mode — mitigate with a README line and
  by adding users as test users; or go through verification if this grows beyond a few.
- **Corp Google Workspace may block third-party OAuth** (admin-restricted API access).
  If the colleague's Workspace admin disallows it, consent fails — same dead-end as
  MDM-blocked CalDAV, but worth knowing up front. Test with her account early.
- **Token revocation / expiry** — handle refresh-token invalidation (user revoked
  access, password change) by dropping to a clean "reconnect Google" state, never a
  silent failure.
- **Privacy optics now real, not just perceived** — calendar data does leave Google
  to the app over HTTPS (though it stays on her Mac after). The honest copy is "Humla
  reads your Google Calendar to know when meetings start and to tag recordings with who
  was there; it stays on this Mac," not "never leaves Google."

## 7. Open questions

- Pre-roll: start a few seconds *before* `event.start` to catch "so as I was saying"
  cold opens? Leaning yes, ~10 s, configurable.
- Should `Ask` remember "always record this recurring series" so a weekly 1:1 becomes
  effectively Auto without global Auto? Nice-to-have, probably M5+.
- Menubar vs in-app only for the live "next meeting" indicator — depends on whether a
  menubar extra is in scope elsewhere.

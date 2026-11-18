//! Onboarding counters — REMOVED in this personal fork.
//!
//! Upstream Parley POSTed anonymous onboarding milestones (first launch, setup
//! finished) to its own server at `sync.humla.team`. This fork is single-user
//! and sends nothing: the command below is a deliberate no-op, kept only so the
//! existing frontend callers in the onboarding flow remain valid without a wider
//! edit. No network call, no stored "sent" markers, no `telemetry_enabled` read.
//!
//! The user-facing "Send anonymous usage counters" toggles (About tab and the
//! onboarding notice) have been removed too, so there is no control implying data
//! leaves the machine.

/// No-op. Accepts the same arguments the frontend already passes and does
/// nothing with them — nothing is sent, nothing is stored.
#[tauri::command]
pub fn telemetry_event(_app: tauri::AppHandle, _event: String) {}

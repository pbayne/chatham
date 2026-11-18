//! Tauri commands for speaker voiceprints (voiceprints Phase D — lifecycle).
//!
//! Two user-facing flows:
//!   - `voiceprint_enroll` — the user confirmed a speaker's name on a recording,
//!     so embed that speaker's segments from the take audio and store the print
//!     under the name. Called after a rename the user made.
//!   - `voiceprint_match_note` — for a note's diarized speakers, embed each and
//!     match against enrolled prints, returning name suggestions with strength
//!     (Auto/Suggest) so the UI can auto-apply or offer a confirm.
//!
//! Both read the per-session `timeline.jsonl` to get a label's ms ranges and the
//! session's `playback.wav` (the mixed stream that carries every speaker).

use crate::{diarize, voiceprints, AppState};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, State};

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// One timeline row we care about: a labelled span in a session's audio.
#[derive(serde::Deserialize)]
struct TimelineRow {
    start_ms: i64,
    end_ms: i64,
    label: String,
}

/// Collect a label's (start_ms, end_ms) ranges across a note's sessions, paired
/// with the session's playback.wav. Returns (wav_path, ranges) per session that
/// has any of the label's audio. Ranges are capped to keep the embed quick and
/// the clip clean (longest turns first, up to ~120s total).
fn label_ranges_by_session(
    app: &AppHandle,
    note_id: &str,
    label: &str,
) -> Vec<(PathBuf, Vec<(i64, i64)>)> {
    let Ok(app_dir) = app.path().app_data_dir() else {
        return vec![];
    };
    let recordings = crate::sessions::recordings_dir(&app_dir, note_id);
    let mut out = vec![];
    for (_, dir) in crate::sessions::resolve_sessions(&recordings) {
        let timeline = dir.join("timeline.jsonl");
        // Prefer compressed `playback.m4a`, fall back to `playback.wav`. The
        // Swift diarizer (AVAudioFile) decodes AAC natively.
        let Some(wav) = crate::sessions::resolve_session_audio(&dir, "playback") else {
            continue;
        };
        if !timeline.exists() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&timeline) else {
            continue;
        };
        let mut ranges: Vec<(i64, i64)> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<TimelineRow>(l).ok())
            .filter(|r| r.label == label && r.end_ms > r.start_ms)
            .map(|r| (r.start_ms, r.end_ms))
            .collect();
        if ranges.is_empty() {
            continue;
        }
        // Longest turns first so the cap keeps the most speech; then back to
        // chronological for a natural clip.
        ranges.sort_by_key(|(a, b)| -(b - a));
        let mut capped = vec![];
        let mut total = 0i64;
        for (a, b) in ranges {
            if total >= 120_000 {
                break;
            }
            total += b - a;
            capped.push((a, b));
        }
        capped.sort_by_key(|(a, _)| *a);
        out.push((wav, capped));
    }
    out
}

/// Embed a label across a note's sessions and return the single best (longest-
/// audio) embedding. One print per enrollment keeps it simple; the store already
/// accumulates across meetings.
async fn embed_label(app: &AppHandle, note_id: &str, label: &str) -> Result<Vec<f32>, String> {
    let by_session = label_ranges_by_session(app, note_id, label);
    if by_session.is_empty() {
        return Err("no audio for that speaker".into());
    }
    // Pick the session with the most of this label's audio.
    let (wav, ranges) = by_session
        .into_iter()
        .max_by_key(|(_, r)| r.iter().map(|(a, b)| b - a).sum::<i64>())
        .unwrap();
    diarize::embed_speaker(app, &wav, &ranges).await.map_err(err)
}

/// Enroll a confirmed name: embed the speaker's segments and store the print.
/// `label` is the diarized label as it appears in the timeline (e.g. "Hege"
/// after a rename, or "Speaker 2" if enrolling before renaming).
#[tauri::command]
pub async fn voiceprint_enroll(
    app: AppHandle,
    note_id: String,
    label: String,
    name: String,
) -> Result<(), String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("name is empty".into());
    }
    let embedding = embed_label(&app, &note_id, &label).await?;
    let state: State<AppState> = app.state();
    let conn = state.db.lock();
    let workspace = crate::commands::cloud::active_workspace(&conn);
    voiceprints::enroll(&conn, &name, &embedding, &workspace, &note_id).map_err(err)?;
    eprintln!("voiceprint: enrolled {name:?} from note {note_id} label {label:?}");
    Ok(())
}

/// A name suggestion for one of a note's diarized labels.
#[derive(Serialize)]
pub struct LabelSuggestion {
    pub label: String,
    pub name: String,
    pub score: f32,
    /// "auto" | "suggest" — mirrors voiceprints::MatchStrength (None filtered out).
    pub strength: String,
}

/// For each diarized label in a note, embed it and match against enrolled
/// prints. Returns only Auto/Suggest hits. The UI auto-applies "auto" and
/// offers "suggest" for confirmation. `labels` are the note's current speaker
/// labels (from the transcript) that are still generic (the caller filters).
#[tauri::command]
pub async fn voiceprint_match_note(
    app: AppHandle,
    note_id: String,
    labels: Vec<String>,
) -> Result<Vec<LabelSuggestion>, String> {
    // Snapshot the enrolled prints once.
    let (prints, _ws) = {
        let state: State<AppState> = app.state();
        let conn = state.db.lock();
        let ws = crate::commands::cloud::active_workspace(&conn);
        (voiceprints::list(&conn, &ws).map_err(err)?, ws)
    };
    if prints.is_empty() {
        return Ok(vec![]);
    }

    let mut out = vec![];
    for label in labels {
        let Ok(embedding) = embed_label(&app, &note_id, &label).await else {
            continue;
        };
        if let Some(m) = voiceprints::best_match(&embedding, &prints, voiceprints::SUGGEST_THRESHOLD) {
            let strength = match voiceprints::strength(m.score) {
                voiceprints::MatchStrength::Auto => "auto",
                voiceprints::MatchStrength::Suggest => "suggest",
                voiceprints::MatchStrength::None => continue,
            };
            out.push(LabelSuggestion {
                label,
                name: m.name,
                score: m.score,
                strength: strength.into(),
            });
        }
    }
    Ok(out)
}

/// Distinct enrolled voiceprint names (for a People/settings view).
#[tauri::command]
pub fn voiceprint_names(state: State<AppState>) -> Result<Vec<String>, String> {
    let conn = state.db.lock();
    let ws = crate::commands::cloud::active_workspace(&conn);
    voiceprints::names(&conn, &ws).map_err(err)
}

/// Forget every print under a name (correction / privacy).
#[tauri::command]
pub fn voiceprint_forget(state: State<AppState>, name: String) -> Result<usize, String> {
    let conn = state.db.lock();
    let ws = crate::commands::cloud::active_workspace(&conn);
    voiceprints::forget(&conn, &ws, &name).map_err(err)
}

// Silence unused-import noise on non-macos builds where Path may be unreferenced.
#[allow(unused)]
fn _assert_path(_: &Path) {}

//! Speaker voiceprints (#voiceprints Phase B) — the name↔voice store and the
//! pure matching math.
//!
//! A voiceprint is an acoustic embedding (a fixed-length Vec<f32> from the
//! speaker-embedding sidecar, Phase C) stored under a name. Enrollment happens
//! when the user confirms a speaker's name on a meeting that has audio: we embed
//! that speaker's segments and save the vector here. On a later meeting we embed
//! each unknown speaker and match against the store by cosine similarity; a
//! match above a threshold suggests the name.
//!
//! This file is split so the MATH is pure and unit-tested without audio or a
//! model: `cosine`, `best_match`, and the CRUD over the `voiceprints` table.
//! The sidecar that produces embeddings is Phase C.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// An enrolled voiceprint row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Voiceprint {
    pub id: String,
    pub name: String,
    pub embedding: Vec<f32>,
    pub workspace_id: String,
    pub source_note_id: String,
    pub created_at: i64,
}

/// A match result: the name and how close (cosine, 0..1) the best enrolled
/// print for that name was.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceMatch {
    pub name: String,
    pub score: f32,
}

/// Cosine thresholds, tuned on real Chatham recordings (per-speaker segment
/// embeddings from the FluidAudio WeSpeaker model, L2-normalized 256-d):
///   - same-speaker self-matches clustered ~0.55–0.80
///   - different speakers clustered ~0.0–0.25
///   - a thin overlap band in between, where a confident auto-label would risk
///     a wrong name — so that band only *suggests*, it never auto-applies.
/// `AUTO` is deliberately high (a wrong auto-rename is worse than no rename);
/// `SUGGEST` opens a review band the UI can surface for the user to confirm.
pub const AUTO_THRESHOLD: f32 = 0.78;
pub const SUGGEST_THRESHOLD: f32 = 0.55;

/// Back-compat alias (older callers): the suggest floor is the general
/// "could be a match at all" line.
pub const DEFAULT_THRESHOLD: f32 = SUGGEST_THRESHOLD;

/// Whether a match score is strong enough to auto-apply the name vs merely
/// suggest it for confirmation vs ignore. Lets the UI pick its behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MatchStrength {
    Auto,
    Suggest,
    None,
}

pub fn strength(score: f32) -> MatchStrength {
    if score >= AUTO_THRESHOLD {
        MatchStrength::Auto
    } else if score >= SUGGEST_THRESHOLD {
        MatchStrength::Suggest
    } else {
        MatchStrength::None
    }
}

/// Cosine similarity of two equal-length vectors. Returns 0.0 for a length
/// mismatch or a zero-magnitude vector (no panic, no NaN leaking out).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    let sim = dot / (na.sqrt() * nb.sqrt());
    // Clamp tiny FP overshoot so callers can trust a clean 0..1 (sign kept).
    sim.clamp(-1.0, 1.0)
}

/// Best-matching enrolled name for `probe`, if any beats `threshold`. When a
/// name has several enrolled prints, its score is the MAX over them (any past
/// encounter matching is enough). Ties broken by higher score, then name.
pub fn best_match(probe: &[f32], prints: &[Voiceprint], threshold: f32) -> Option<VoiceMatch> {
    let mut best: Option<VoiceMatch> = None;
    for p in prints {
        let score = cosine(probe, &p.embedding);
        if score < threshold {
            continue;
        }
        let better = match &best {
            None => true,
            Some(b) => score > b.score || (score == b.score && p.name < b.name),
        };
        if better {
            best = Some(VoiceMatch { name: p.name.clone(), score });
        }
    }
    best
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Enroll a voiceprint: store `embedding` under `name`. Each call adds a row, so
/// repeat encounters accumulate and refine future matching. Returns the row.
pub fn enroll(
    conn: &Connection,
    name: &str,
    embedding: &[f32],
    workspace_id: &str,
    source_note_id: &str,
) -> rusqlite::Result<Voiceprint> {
    let id = format!("vp-{}", uuid_like());
    let created_at = now_secs();
    let embedding_json = serde_json::to_string(embedding).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO voiceprints (id, name, embedding, workspace_id, source_note_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, name, embedding_json, workspace_id, source_note_id, created_at],
    )?;
    Ok(Voiceprint {
        id,
        name: name.to_string(),
        embedding: embedding.to_vec(),
        workspace_id: workspace_id.to_string(),
        source_note_id: source_note_id.to_string(),
        created_at,
    })
}

/// Every enrolled voiceprint in a workspace.
pub fn list(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<Voiceprint>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, embedding, workspace_id, source_note_id, created_at
         FROM voiceprints WHERE workspace_id = ?1 ORDER BY name, created_at",
    )?;
    let rows = stmt.query_map(params![workspace_id], map_row)?;
    rows.collect()
}

/// Distinct enrolled names in a workspace (for the People directory / UI).
pub fn names(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT name FROM voiceprints WHERE workspace_id = ?1 ORDER BY name",
    )?;
    let rows = stmt.query_map(params![workspace_id], |r| r.get::<_, String>(0))?;
    rows.collect()
}

/// Forget every voiceprint under a name (user correction / privacy).
pub fn forget(conn: &Connection, workspace_id: &str, name: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM voiceprints WHERE workspace_id = ?1 AND name = ?2",
        params![workspace_id, name],
    )
}

fn map_row(r: &rusqlite::Row) -> rusqlite::Result<Voiceprint> {
    let embedding_json: String = r.get(2)?;
    Ok(Voiceprint {
        id: r.get(0)?,
        name: r.get(1)?,
        embedding: serde_json::from_str(&embedding_json).unwrap_or_default(),
        workspace_id: r.get(3)?,
        source_note_id: r.get(4)?,
        created_at: r.get(5)?,
    })
}

/// A compact random id without pulling a uuid crate: time + a counter-ish rand.
fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp(name: &str, e: Vec<f32>) -> Voiceprint {
        Voiceprint {
            id: format!("id-{name}"),
            name: name.into(),
            embedding: e,
            workspace_id: String::new(),
            source_note_id: String::new(),
            created_at: 0,
        }
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine(&[1.0, 1.0], &[2.0, 2.0]) - 1.0).abs() < 1e-6); // scale-invariant
    }

    #[test]
    fn cosine_guards_bad_input() {
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0, 2.0], &[1.0]), 0.0); // length mismatch
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0); // zero magnitude
    }

    #[test]
    fn best_match_picks_closest_above_threshold() {
        let prints = vec![
            vp("Hege", vec![1.0, 0.0, 0.0]),
            vp("Michael", vec![0.0, 1.0, 0.0]),
        ];
        let m = best_match(&[0.95, 0.05, 0.0], &prints, DEFAULT_THRESHOLD).unwrap();
        assert_eq!(m.name, "Hege");
        assert!(m.score > 0.9);
    }

    #[test]
    fn best_match_returns_none_below_threshold() {
        let prints = vec![vp("Hege", vec![1.0, 0.0, 0.0])];
        // Orthogonal-ish probe, score ~0 < 0.75.
        assert!(best_match(&[0.0, 1.0, 0.0], &prints, DEFAULT_THRESHOLD).is_none());
    }

    #[test]
    fn best_match_takes_max_over_a_names_prints() {
        let prints = vec![
            vp("Hege", vec![0.0, 1.0, 0.0]), // far from probe
            vp("Hege", vec![1.0, 0.0, 0.0]), // close to probe
        ];
        let m = best_match(&[1.0, 0.0, 0.0], &prints, DEFAULT_THRESHOLD).unwrap();
        assert_eq!(m.name, "Hege");
        assert!((m.score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn strength_bands() {
        assert_eq!(strength(0.90), MatchStrength::Auto);
        assert_eq!(strength(0.78), MatchStrength::Auto);
        assert_eq!(strength(0.65), MatchStrength::Suggest);
        assert_eq!(strength(0.55), MatchStrength::Suggest);
        assert_eq!(strength(0.40), MatchStrength::None);
    }

    #[test]
    fn enroll_list_forget_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("vp.sqlite")).unwrap();
        enroll(&conn, "Hege", &[1.0, 0.0], "ws", "n1").unwrap();
        enroll(&conn, "Hege", &[0.9, 0.1], "ws", "n2").unwrap();
        enroll(&conn, "Michael", &[0.0, 1.0], "ws", "n1").unwrap();
        assert_eq!(list(&conn, "ws").unwrap().len(), 3);
        assert_eq!(names(&conn, "ws").unwrap(), vec!["Hege", "Michael"]);
        assert_eq!(forget(&conn, "ws", "Hege").unwrap(), 2);
        assert_eq!(names(&conn, "ws").unwrap(), vec!["Michael"]);
    }
}

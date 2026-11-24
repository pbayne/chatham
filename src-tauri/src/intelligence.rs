//! Cross-note aggregation for the meeting-intelligence views (roadmap Phase C).
//!
//! Two read-only rollups built on existing retrieval:
//!   - PERSON: everywhere a speaker appears across notes, their open
//!     commitments, recurring topics, and when they were last seen.
//!   - CLIENT: a client's notes newest-first with summaries + open items — the
//!     raw material Feature 3's "what changed since last call" digest
//!     summarizes.
//!
//! The DB access is a thin call to `db::list_notes_filtered` (which already
//! supports both a `speaker` and a `client_id` filter) plus per-note open
//! action items. Everything derived — date range, last-seen, topic counts — is
//! computed by the PURE functions here, so it's unit-testable without a DB.

use crate::db::{ActionItem, NoteMeta};
use std::collections::HashMap;

/// A note a person or client appears in, flattened to what the views show.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppearanceNote {
    pub note_id: String,
    pub title: String,
    pub created_at: i64,
    pub summary: String,
    /// Open (not-done) action items on this note owned by the person, or all
    /// open items for a client rollup.
    pub open_items: Vec<ActionItem>,
}

/// A person's cross-note activity (Feature 1).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PersonActivity {
    pub name: String,
    pub note_count: usize,
    /// Epoch-ms of the earliest and latest note they appear in (None when zero).
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    /// Their still-open commitments across all notes, newest note first.
    pub open_items: Vec<ActionItem>,
    /// Recurring single-word topics from note titles + summaries, most frequent
    /// first (stopworded, min length). A cheap signal, not a model call.
    pub topics: Vec<(String, usize)>,
    pub notes: Vec<AppearanceNote>,
}

/// A client's cross-note activity (Feature 3 input).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClientActivity {
    pub client_id: String,
    pub client_name: String,
    pub note_count: usize,
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    pub open_items: Vec<ActionItem>,
    pub topics: Vec<(String, usize)>,
    pub notes: Vec<AppearanceNote>,
}

/// Earliest and latest `created_at` across notes (notes need not be sorted).
pub fn date_range(notes: &[NoteMeta]) -> (Option<i64>, Option<i64>) {
    let mut min = None;
    let mut max = None;
    for n in notes {
        min = Some(min.map_or(n.created_at, |m: i64| m.min(n.created_at)));
        max = Some(max.map_or(n.created_at, |m: i64| m.max(n.created_at)));
    }
    (min, max)
}

/// A speaker matches a name when their label equals it OR (for the user's own
/// speech) equals the "You" sentinel the caller passes as `alias`. Exact, never
/// substring — "Michael" must not match "Michael Berg" (merging two people).
pub fn note_has_person(note: &NoteMeta, name: &str, alias: Option<&str>) -> bool {
    note.speakers
        .iter()
        .any(|s| s == name || alias.is_some_and(|a| s == a))
}

/// Recurring topics from titles + summaries: lowercased word counts over a
/// stopword list, keeping words ≥4 chars seen ≥2 times, most frequent first,
/// capped. Deterministic tie-break by the word itself so output is stable.
pub fn topics(notes: &[NoteMeta], cap: usize) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for n in notes {
        let mut text = n.title.clone();
        text.push(' ');
        text.push_str(&n.summary);
        for raw in text.split(|c: char| !c.is_alphanumeric()) {
            let w = raw.to_ascii_lowercase();
            if w.len() < 4 || is_topic_stopword(&w) {
                continue;
            }
            *counts.entry(w).or_insert(0) += 1;
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().filter(|(_, c)| *c >= 2).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(cap);
    v
}

fn is_topic_stopword(w: &str) -> bool {
    matches!(
        w,
        "this" | "that" | "with" | "from" | "have" | "will" | "about" | "there"
            | "their" | "would" | "could" | "should" | "which" | "what" | "when"
            | "then" | "than" | "they" | "them" | "were" | "been" | "into" | "some"
            | "more" | "just" | "like" | "also" | "only" | "over" | "your" | "ours"
            | "meeting" | "call" | "note" | "notes" | "discussion" | "talked"
            | "going" | "really" | "think" | "thing" | "things" | "stuff"
    )
}

/// Assemble a `PersonActivity` from the person's notes (already filtered to
/// ones they appear in) and a map of note_id → that note's open action items
/// owned by the person. Pure: the DB layer supplies both inputs.
pub fn person_activity(
    name: &str,
    mut notes: Vec<NoteMeta>,
    open_by_note: &HashMap<String, Vec<ActionItem>>,
) -> PersonActivity {
    notes.sort_by(|a, b| b.created_at.cmp(&a.created_at)); // newest first
    let (first_seen, last_seen) = date_range(&notes);
    let topics = topics(&notes, 8);
    let mut open_items = Vec::new();
    let appearance: Vec<AppearanceNote> = notes
        .iter()
        .map(|n| {
            let items = open_by_note.get(&n.id).cloned().unwrap_or_default();
            open_items.extend(items.iter().cloned());
            AppearanceNote {
                note_id: n.id.clone(),
                title: n.title.clone(),
                created_at: n.created_at,
                summary: n.summary.clone(),
                open_items: items,
            }
        })
        .collect();
    PersonActivity {
        name: name.to_string(),
        note_count: notes.len(),
        first_seen,
        last_seen,
        open_items,
        topics,
        notes: appearance,
    }
}

/// Assemble a `ClientActivity` the same way, for a client's notes.
pub fn client_activity(
    client_id: &str,
    client_name: &str,
    mut notes: Vec<NoteMeta>,
    open_by_note: &HashMap<String, Vec<ActionItem>>,
) -> ClientActivity {
    notes.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let (first_seen, last_seen) = date_range(&notes);
    let topics = topics(&notes, 8);
    let mut open_items = Vec::new();
    let appearance: Vec<AppearanceNote> = notes
        .iter()
        .map(|n| {
            let items = open_by_note.get(&n.id).cloned().unwrap_or_default();
            open_items.extend(items.iter().cloned());
            AppearanceNote {
                note_id: n.id.clone(),
                title: n.title.clone(),
                created_at: n.created_at,
                summary: n.summary.clone(),
                open_items: items,
            }
        })
        .collect();
    ClientActivity {
        client_id: client_id.to_string(),
        client_name: client_name.to_string(),
        note_count: notes.len(),
        first_seen,
        last_seen,
        open_items,
        topics,
        notes: appearance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, title: &str, created: i64, summary: &str, speakers: &[&str]) -> NoteMeta {
        NoteMeta {
            id: id.into(),
            title: title.into(),
            created_at: created,
            folder_id: None,
            client_id: None,
            client_name: None,
            summary: summary.into(),
            speakers: speakers.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn date_range_finds_min_max() {
        let notes = vec![
            meta("a", "A", 300, "", &[]),
            meta("b", "B", 100, "", &[]),
            meta("c", "C", 200, "", &[]),
        ];
        assert_eq!(date_range(&notes), (Some(100), Some(300)));
        assert_eq!(date_range(&[]), (None, None));
    }

    #[test]
    fn person_match_is_exact_with_alias() {
        let n = meta("a", "A", 1, "", &["Michael Berg", "You"]);
        assert!(note_has_person(&n, "Michael Berg", None));
        assert!(!note_has_person(&n, "Michael", None)); // no substring merge
        assert!(note_has_person(&n, "Patrick", Some("You"))); // sentinel alias
        assert!(!note_has_person(&n, "Patrick", None));
    }

    #[test]
    fn topics_counts_recurring_words_only() {
        let notes = vec![
            meta("a", "Pricing for Acme", 1, "We discussed pricing and renewal terms", &[]),
            meta("b", "Renewal plan", 2, "renewal timeline and pricing tiers", &[]),
        ];
        let t = topics(&notes, 8);
        // "pricing" (3) and "renewal" (3) recur; "meeting"/"call" stopworded;
        // short words dropped.
        let words: Vec<&str> = t.iter().map(|(w, _)| w.as_str()).collect();
        assert!(words.contains(&"pricing"));
        assert!(words.contains(&"renewal"));
        // frequency order: both appear 3x; tie broken alphabetically → pricing first
        assert_eq!(t[0].0, "pricing");
    }

    #[test]
    fn person_activity_sorts_newest_first_and_gathers_open_items() {
        let notes = vec![
            meta("old", "Old", 100, "", &["Sarah"]),
            meta("new", "New", 500, "", &["Sarah"]),
        ];
        let mut open = HashMap::new();
        open.insert(
            "new".to_string(),
            vec![ActionItem {
                id: "i1".into(),
                note_id: "new".into(),
                text: "Send SOW".into(),
                owner: "Sarah".into(),
                due: "2026-03-13".into(),
                done: false,
                source: "auto".into(),
                sort_order: 0,
                created_at: 500,
                updated_at: 500,
            }],
        );
        let pa = person_activity("Sarah", notes, &open);
        assert_eq!(pa.note_count, 2);
        assert_eq!(pa.first_seen, Some(100));
        assert_eq!(pa.last_seen, Some(500));
        assert_eq!(pa.notes[0].note_id, "new"); // newest first
        assert_eq!(pa.open_items.len(), 1);
        assert_eq!(pa.open_items[0].text, "Send SOW");
    }
}

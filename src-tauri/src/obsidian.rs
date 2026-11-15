//! Obsidian vault sync — a one-way mirror of Humla notes into a user-chosen
//! vault folder as readable Markdown. Ported in spirit from stenoai's
//! `obsidian-sync.js`, adapted to Humla's SQLite note store.
//!
//! Design contract (kept identical to the proven stenoai version):
//!   - **DB IS THE SOURCE OF TRUTH.** A note's row drives its vault copy; the
//!     sync re-reads and reconciles. One code path for every trigger.
//!   - **NEVER PANIC INTO A CALLER.** A failing vault write (unplugged drive,
//!     cloud-folder conflict, read-only) must never break a note save. Every
//!     entry point returns a status; nothing propagates an fs error up.
//!   - **ONE-WAY.** Obsidian-side edits are never read back. If the vault copy
//!     was edited externally (its bytes differ from what we last wrote), SKIP
//!     the overwrite and flag a conflict rather than clobber the user's edit.
//!   - **Identity is the note id** (stable across title/folder changes); the
//!     index maps id -> vault path so a rename moves the file, not orphans it.
//!   - **Off by default:** `sync_note` is a no-op unless enabled AND a vault
//!     path is set, so the note lifecycle pays nothing when the feature is off.
//!
//! The transcript is intentionally dropped from the vault copy (summary + typed
//! notes only), matching stenoai and keeping vaults readable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::{Folder, Note};

const SETTING_ENABLED: &str = "obsidian_sync_enabled";
const SETTING_VAULT: &str = "obsidian_vault_path";
const STATE_FILE: &str = ".obsidian-sync-state.json";

/// Per-note record of what we last wrote, so an external edit is detectable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    vault_rel_path: String,
    last_written_hash: String,
    last_synced_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SyncState {
    #[serde(default)]
    notes: HashMap<String, Entry>,
    #[serde(default)]
    conflicts: HashMap<String, String>,
}

/// Outcome of syncing one note, surfaced to the UI for a status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    Disabled,
    Synced,
    Conflict,
    Skipped,
    Removed,
    Absent,
    Error,
}

// ─── Pure helpers (unit-tested; no fs) ───────────────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// Filesystem-safe filename segment for macOS + Windows. Never returns
/// ""/"."/"..", so a folder named ".." can't let a write escape the vault via
/// join. Strips leading/trailing dots+spaces, collapses whitespace, caps length.
pub fn sanitize_filename(name: &str, fallback: &str) -> String {
    let clean = |v: &str| -> String {
        let replaced: String = v
            .chars()
            .map(|c| match c {
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
                c if (c as u32) < 0x20 => ' ',
                c => c,
            })
            .collect();
        replaced
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_matches(|c: char| c == '.' || c.is_whitespace())
            .to_string()
    };
    let mut s = clean(name);
    if s.is_empty() {
        s = clean(fallback);
    }
    if s.is_empty() {
        s = "_".to_string();
    }
    // Windows reserved device names.
    let lower = s.to_ascii_lowercase();
    let reserved = ["con", "prn", "aux", "nul"]
        .iter()
        .any(|r| lower == *r || lower.starts_with(&format!("{r}.")))
        || ((lower.starts_with("com") || lower.starts_with("lpt"))
            && lower.len() >= 4
            && lower.as_bytes()[3].is_ascii_digit());
    if reserved {
        s = format!("_{s}");
    }
    if s.chars().count() > 180 {
        s = s.chars().take(180).collect::<String>().trim_end_matches(['.', ' ']).to_string();
        if s.is_empty() {
            s = "_".to_string();
        }
    }
    s
}

/// `YYYY-MM-DD` from a unix-seconds timestamp (UTC). Empty on a nonsensical ts.
fn date_str(created_at_secs: i64) -> String {
    if created_at_secs <= 0 {
        return String::new();
    }
    // Civil-from-days (Howard Hinnant's algorithm), UTC.
    let days = created_at_secs.div_euclid(86400);
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
    format!("{year:04}-{m:02}-{d:02}")
}

fn yaml_quote(v: &str) -> String {
    let escaped = v.replace('\\', "\\\\").replace('"', "\\\"").replace(['\n', '\r'], " ");
    format!("\"{escaped}\"")
}

/// Build the Markdown the vault copy should contain, from a note + its folder
/// name. Frontmatter (title/date/folder/source/humla_id) + summary + typed
/// notes. Transcript deliberately omitted.
pub fn render_vault_markdown(note: &Note, folder_name: Option<&str>, date: &str) -> String {
    let mut fm = vec!["---".to_string(), format!("title: {}", yaml_quote(&note.title))];
    if !date.is_empty() {
        fm.push(format!("date: {date}"));
    }
    if let Some(f) = folder_name {
        if !f.is_empty() {
            fm.push(format!("folder: {}", yaml_quote(f)));
        }
    }
    fm.push("source: Humla".to_string());
    fm.push(format!("humla_id: {}", yaml_quote(&note.id)));
    fm.push("---".to_string());

    let mut out = fm.join("\n");
    out.push_str("\n\n");
    out.push_str(&format!("# {}\n", note.title));

    let summary = note.summary.trim();
    if !summary.is_empty() {
        out.push_str("\n## Summary\n\n");
        out.push_str(summary);
        out.push('\n');
    }
    let body = note.body.trim();
    if !body.is_empty() {
        out.push_str("\n## Notes\n\n");
        out.push_str(body);
        out.push('\n');
    }
    out
}

/// `YYYY-MM-DD Title.md`, disambiguated by id when a *different* note already
/// owns the plain name. `is_taken` reports a collision for a candidate.
pub fn derive_filename(date: &str, title: &str, id: &str, is_taken: impl Fn(&str) -> bool) -> String {
    let base = [date, &sanitize_filename(title, id)]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let short = id.chars().take(8).collect::<String>();
    let mut name = format!("{base}.md");
    if is_taken(&name) {
        name = format!("{base} ({short}).md");
        let mut i = 2;
        while is_taken(&name) {
            name = format!("{base} ({short}-{i}).md");
            i += 1;
        }
    }
    name
}

// ─── Stateful engine (fs) ────────────────────────────────────────────────────

/// Read the sync config from settings. Active only when enabled + a vault path.
pub fn config(conn: &rusqlite::Connection) -> (bool, String) {
    let enabled = crate::db::get_setting(conn, SETTING_ENABLED)
        .ok()
        .flatten()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let vault = crate::db::get_setting(conn, SETTING_VAULT).ok().flatten().unwrap_or_default();
    (enabled, vault)
}

pub fn is_active(conn: &rusqlite::Connection) -> bool {
    let (enabled, vault) = config(conn);
    enabled && !vault.trim().is_empty()
}

/// (synced, conflict) counts from the vault state file. Best-effort: (0,0) when
/// there's no vault or the state can't be read.
pub fn counts(vault: &str) -> (usize, usize) {
    if vault.trim().is_empty() {
        return (0, 0);
    }
    let state = load_state(vault);
    (state.notes.len(), state.conflicts.len())
}

fn state_path(vault: &str) -> PathBuf {
    Path::new(vault).join(STATE_FILE)
}

fn load_state(vault: &str) -> SyncState {
    std::fs::read_to_string(state_path(vault))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(vault: &str, state: &SyncState) {
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = atomic_write(&state_path(vault), json.as_bytes());
    }
}

/// Write `data` to `dest` atomically (temp file + rename on the same volume).
fn atomic_write(dest: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, dest)
}

fn is_externally_edited(abs: &Path, entry: &Entry) -> bool {
    match std::fs::read(abs) {
        Ok(bytes) => sha256_hex(&bytes) != entry.last_written_hash,
        Err(_) => false, // gone → not a conflict, we'll recreate
    }
}

/// Sync one note's vault copy from the DB. The folder list resolves the note's
/// folder id to a name for the subfolder. Returns a status; never panics.
pub fn sync_note(conn: &rusqlite::Connection, note: &Note, folders: &[Folder]) -> SyncStatus {
    let (enabled, vault) = config(conn);
    if !enabled || vault.trim().is_empty() {
        return SyncStatus::Disabled;
    }
    let mut state = load_state(&vault);
    let status = sync_one(&vault, note, folders, &mut state);
    save_state(&vault, &state);
    status
}

fn sync_one(vault: &str, note: &Note, folders: &[Folder], state: &mut SyncState) -> SyncStatus {
    let folder_name = note
        .folder_id
        .as_ref()
        .and_then(|fid| folders.iter().find(|f| &f.id == fid))
        .map(|f| f.name.clone());
    let date = date_str(note.created_at);
    let body = render_vault_markdown(note, folder_name.as_deref(), &date);
    let new_hash = sha256_hex(body.as_bytes());

    let sub = folder_name
        .as_deref()
        .map(|f| sanitize_filename(f, f))
        .unwrap_or_default();

    let existing = state.notes.get(&note.id).cloned();
    let taken = |name: &str| -> bool {
        let rel = if sub.is_empty() { name.to_string() } else { format!("{sub}/{name}") };
        // Another tracked note already owns this path?
        if state
            .notes
            .iter()
            .any(|(id, e)| id != &note.id && e.vault_rel_path == rel)
        {
            return true;
        }
        // A pre-existing untracked file is the user's own — never clobber it.
        let owned_by_us = existing.as_ref().map(|e| e.vault_rel_path == rel).unwrap_or(false);
        !owned_by_us && Path::new(vault).join(&rel).exists()
    };
    let filename = derive_filename(&date, &note.title, &note.id, taken);
    let vault_rel = if sub.is_empty() { filename.clone() } else { format!("{sub}/{filename}") };
    let abs_dest = Path::new(vault).join(&vault_rel);

    // The note moved (title/folder changed): remove the old-name copy unless the
    // user edited it, then the write below recreates it at the new path.
    if let Some(entry) = &existing {
        if entry.vault_rel_path != vault_rel {
            let abs_old = Path::new(vault).join(&entry.vault_rel_path);
            if is_externally_edited(&abs_old, entry) {
                state.conflicts.insert(note.id.clone(), entry.vault_rel_path.clone());
                return SyncStatus::Conflict;
            }
            let _ = std::fs::remove_file(&abs_old);
            if let Some(dir) = abs_old.parent() {
                let _ = std::fs::remove_dir(dir); // best-effort, only if empty
            }
        }
    }

    let dest_hash = std::fs::read(&abs_dest).ok().map(|b| sha256_hex(&b));

    // Already identical on disk: skip the write (no mtime churn for cloud vaults).
    if dest_hash.as_deref() == Some(new_hash.as_str()) {
        state.notes.insert(
            note.id.clone(),
            Entry { vault_rel_path: vault_rel, last_written_hash: new_hash, last_synced_at: now_secs() },
        );
        state.conflicts.remove(&note.id);
        return SyncStatus::Synced;
    }

    // Conflict: a tracked destination was edited in Obsidian since our last write.
    if dest_hash.is_some() {
        if let Some(entry) = &existing {
            if is_externally_edited(&abs_dest, entry) {
                state.conflicts.insert(note.id.clone(), vault_rel.clone());
                return SyncStatus::Conflict;
            }
        } else {
            // Untracked pre-existing file at our target — treat as the user's.
            state.conflicts.insert(note.id.clone(), vault_rel.clone());
            return SyncStatus::Conflict;
        }
    }

    if atomic_write(&abs_dest, body.as_bytes()).is_err() {
        return SyncStatus::Error;
    }
    state.notes.insert(
        note.id.clone(),
        Entry { vault_rel_path: vault_rel, last_written_hash: new_hash, last_synced_at: now_secs() },
    );
    state.conflicts.remove(&note.id);
    SyncStatus::Synced
}

/// Remove a note's vault copy on delete. Preserves an externally-edited copy.
pub fn remove_note(conn: &rusqlite::Connection, note_id: &str) -> SyncStatus {
    let (enabled, vault) = config(conn);
    if !enabled || vault.trim().is_empty() {
        return SyncStatus::Disabled;
    }
    let mut state = load_state(&vault);
    let Some(entry) = state.notes.get(note_id).cloned() else {
        return SyncStatus::Absent;
    };
    let abs = Path::new(&vault).join(&entry.vault_rel_path);
    if is_externally_edited(&abs, &entry) {
        state.conflicts.insert(note_id.to_string(), entry.vault_rel_path.clone());
        save_state(&vault, &state);
        return SyncStatus::Conflict;
    }
    let _ = std::fs::remove_file(&abs);
    if let Some(dir) = abs.parent() {
        let _ = std::fs::remove_dir(dir);
    }
    state.notes.remove(note_id);
    state.conflicts.remove(note_id);
    save_state(&vault, &state);
    SyncStatus::Removed
}

/// Export every live note when the user turns sync on (or switches vault).
pub fn backfill(conn: &rusqlite::Connection, notes: &[Note], folders: &[Folder]) -> usize {
    let (enabled, vault) = config(conn);
    if !enabled || vault.trim().is_empty() {
        return 0;
    }
    let mut state = load_state(&vault);
    let mut count = 0;
    for note in notes {
        if note.deleted_at.is_some() {
            continue;
        }
        let st = sync_one(&vault, note, folders, &mut state);
        if matches!(st, SyncStatus::Synced) {
            count += 1;
        }
    }
    save_state(&vault, &state);
    count
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str) -> Note {
        Note {
            id: id.into(),
            title: title.into(),
            body: "Typed note body".into(),
            transcript: "SECRET TRANSCRIPT should not reach the vault".into(),
            summary: "## Key points\n- A\n- B".into(),
            audio_path: None,
            summary_preset: "meeting".into(),
            folder_id: None,
            client_id: None,
            language: "en".into(),
            summary_provider: String::new(),
            expected_speakers: None,
            detected_language: None,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
            owner: String::new(),
            workspace_id: String::new(),
            private: false,
            deleted_at: None,
        }
    }

    #[test]
    fn date_str_is_utc_ymd() {
        assert_eq!(date_str(1_700_000_000), "2023-11-14");
        assert_eq!(date_str(0), "");
    }

    #[test]
    fn sanitize_strips_separators_and_dots() {
        assert_eq!(sanitize_filename("a/b:c", "x"), "a b c");
        assert_eq!(sanitize_filename("..", "fallback"), "fallback");
        assert_eq!(sanitize_filename("", "fallback"), "fallback");
        assert_eq!(sanitize_filename("   ", ""), "_");
    }

    #[test]
    fn sanitize_guards_windows_reserved() {
        assert_eq!(sanitize_filename("con", "x"), "_con");
        assert_eq!(sanitize_filename("COM1", "x"), "_COM1");
        assert_eq!(sanitize_filename("console", "x"), "console"); // not reserved
    }

    #[test]
    fn transcript_never_reaches_vault() {
        let md = render_vault_markdown(&note("n1", "Standup"), None, "2023-11-14");
        assert!(!md.contains("SECRET TRANSCRIPT"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("## Notes"));
        assert!(md.contains("humla_id: \"n1\""));
        assert!(md.contains("title: \"Standup\""));
    }

    #[test]
    fn filename_disambiguates_on_collision() {
        let n = note("abcd1234ef", "Weekly");
        // First note takes the plain name.
        let f1 = derive_filename("2023-11-14", &n.title, &n.id, |_| false);
        assert_eq!(f1, "2023-11-14 Weekly.md");
        // A different note colliding gets the id suffix.
        let f2 = derive_filename("2023-11-14", "Weekly", "zzzz9999", |name| name == "2023-11-14 Weekly.md");
        assert_eq!(f2, "2023-11-14 Weekly (zzzz9999).md");
    }

    #[test]
    fn folder_absent_means_no_subdir_segment() {
        // render doesn't place the file; just confirm folder omitted from frontmatter.
        let md = render_vault_markdown(&note("n1", "T"), None, "2023-11-14");
        assert!(!md.contains("folder:"));
        let md2 = render_vault_markdown(&note("n1", "T"), Some("Clients"), "2023-11-14");
        assert!(md2.contains("folder: \"Clients\""));
    }
}

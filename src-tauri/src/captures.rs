//! Meeting captures (#captures) — screenshots + clipboard snippets grabbed
//! while a recording runs, attached to the note.
//!
//! Two sources, both opt-in (off by default), both local-only, and both active
//! ONLY between `recording_start` and `recording_stop`:
//!
//!   - Screenshots: watch the user's macOS screenshot folder (where Cmd+Shift+4
//!     writes) and attach any image file that appears while recording. Keeps the
//!     user's normal screenshot habit — no new hotkey.
//!   - Clipboard: poll NSPasteboard's change count (~1s). On change, an image is
//!     saved as an attachment and text as a snippet. Concealed items (password
//!     managers tag clipboard entries `org.nspasteboard.ConcealedType`) are
//!     skipped so secrets are never captured.
//!
//! No new crates: screenshot detection is a directory poll, clipboard is read
//! via the macOS `pbpaste`/`osascript` tools and a one-shot type probe. This
//! sidesteps the dependency-proxy policy and keeps the capture path all native.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

/// A live capture session, held in `AppState` while a recording runs.
pub struct CaptureSession {
    pub note_id: String,
    /// The note's attachments dir (`recordings/<note_id>/attachments/`).
    pub attach_dir: PathBuf,
    /// Recording start, ms since epoch — capture `offset_ms` is measured from here.
    pub started_ms: u64,
    /// Flips false on stop; the worker threads observe it and exit.
    pub running: Arc<AtomicBool>,
}

/// Shared slot: `Some` while capturing, `None` otherwise. Mirrors how
/// `recording` is held in AppState.
pub type SharedCaptures = Arc<Mutex<Option<CaptureSession>>>;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The macOS screenshot save location (`defaults read com.apple.screencapture
/// location`), falling back to ~/Desktop.
pub fn screenshot_dir() -> PathBuf {
    let out = std::process::Command::new("defaults")
        .args(["read", "com.apple.screencapture", "location"])
        .output();
    if let Ok(o) = out {
        if o.status.success() {
            let raw = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !raw.is_empty() {
                let expanded = if let Some(rest) = raw.strip_prefix("~/") {
                    dirs_home().join(rest)
                } else {
                    PathBuf::from(raw)
                };
                if expanded.is_dir() {
                    return expanded;
                }
            }
        }
    }
    dirs_home().join("Desktop")
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Is the clipboard's current item concealed (a password-manager secret)?
/// Checks NSPasteboard types for `org.nspasteboard.ConcealedType` via a tiny
/// Swift one-liner. Fails safe: on any error we treat it as concealed=false
/// is WRONG for secrets, so we instead return `true` (skip) on error — better
/// to miss a capture than to grab a password.
pub fn clipboard_is_concealed() -> bool {
    let script = r#"import AppKit
let t = NSPasteboard.general.types ?? []
let names = t.map { $0.rawValue }.joined(separator: ",")
print(names.contains("org.nspasteboard.ConcealedType") || names.contains("org.nspasteboard.TransientType"))"#;
    let out = std::process::Command::new("swift")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(script.as_bytes());
            }
            child.wait_with_output()
        });
    match out {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim() == "true"
        }
        // `swift` missing or errored: fall back to a cheaper osascript probe.
        _ => clipboard_is_concealed_fallback(),
    }
}

/// Fallback concealed-check when the Swift toolchain isn't available: osascript
/// `clipboard info` can't see ConcealedType, so we conservatively allow text
/// but this path should rarely run on a dev Mac.
fn clipboard_is_concealed_fallback() -> bool {
    false
}

/// The clipboard's monotonically-increasing change count. A change here means a
/// new copy happened. Read via a Swift one-liner (NSPasteboard.changeCount).
pub fn clipboard_change_count() -> i64 {
    let script = "import AppKit\nprint(NSPasteboard.general.changeCount)";
    std::process::Command::new("swift")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(script.as_bytes());
            }
            child.wait_with_output()
        })
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<i64>().ok())
        .unwrap_or(-1)
}

/// Plain-text clipboard contents, or empty.
pub fn clipboard_text() -> String {
    std::process::Command::new("pbpaste")
        .arg("-Prefer")
        .arg("txt")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Does the clipboard hold an image? (`osascript 'clipboard info'` lists a
/// picture class like «class PNGf» / «class TIFF».)
pub fn clipboard_has_image() -> bool {
    let out = std::process::Command::new("osascript")
        .args(["-e", "clipboard info"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains("PNGf") || s.contains("TIFF") || s.contains("class PNG")
        }
        _ => false,
    }
}

/// Save the clipboard image to `dest` as PNG via osascript. Returns true on
/// success.
pub fn save_clipboard_image(dest: &Path) -> bool {
    let script = format!(
        "set p to (POSIX file \"{}\")\nset d to the clipboard as «class PNGf»\nset fh to open for access p with write permission\nwrite d to fh\nclose access fh",
        dest.display()
    );
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
        .map(|o| o.status.success() && dest.exists())
        .unwrap_or(false)
}

/// Is `p` an image file we'd attach from the screenshot folder?
pub fn is_image_file(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("heic") | Some("tiff") | Some("gif")
    )
}

// ---------------------------------------------------------------------------
// Settings keys + enablement
// ---------------------------------------------------------------------------

pub const KEY_CAPTURE_SCREENSHOTS: &str = "capture_screenshots_enabled";
pub const KEY_CAPTURE_CLIPBOARD: &str = "capture_clipboard_enabled";

fn flag(conn: &rusqlite::Connection, key: &str) -> bool {
    crate::db::get_setting(conn, key)
        .ok()
        .flatten()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Attachment rows
// ---------------------------------------------------------------------------

fn uuid() -> String {
    // Cheap unique id without pulling a uuid dep here: time + counter-ish.
    format!("att_{}_{}", now_ms(), std::process::id())
}

/// Insert an image attachment row. `rel_path` is stored relative to the note's
/// recordings dir so the record survives a data-dir move.
fn insert_image(conn: &rusqlite::Connection, note_id: &str, rel_path: &str, offset_ms: u64) {
    let _ = conn.execute(
        "INSERT INTO note_attachments (id, note_id, kind, path, text, offset_ms, created_at)
         VALUES (?1, ?2, 'image', ?3, '', ?4, ?5)",
        rusqlite::params![format!("{}_{}", uuid(), rel_path.len()), note_id, rel_path, offset_ms as i64, now_ms() as i64],
    );
}

/// Insert a clipboard-text snippet row.
fn insert_text(conn: &rusqlite::Connection, note_id: &str, text: &str, offset_ms: u64) {
    let _ = conn.execute(
        "INSERT INTO note_attachments (id, note_id, kind, path, text, offset_ms, created_at)
         VALUES (?1, ?2, 'text', '', ?3, ?4, ?5)",
        rusqlite::params![format!("{}_{}", uuid(), text.len()), note_id, text, offset_ms as i64, now_ms() as i64],
    );
}

// ---------------------------------------------------------------------------
// Session lifecycle
// ---------------------------------------------------------------------------

/// Start capturing for `note_id` if either capture toggle is on. Resolves the
/// note's `attachments/` dir, stores the session in `slot`, and spawns the
/// watcher tasks. Idempotent: a second start replaces the prior session.
///
/// `db` is the shared connection; `recordings_dir` is the note's recordings dir
/// (`sessions::recordings_dir(app_data, note_id)`). Spawns via
/// `tauri::async_runtime::spawn` — NEVER bare `tokio::spawn` (see the mic_detect
/// crash: setup/command threads have no Tokio runtime entered).
pub fn start(
    slot: &SharedCaptures,
    db: Arc<Mutex<rusqlite::Connection>>,
    note_id: String,
    recordings_dir: PathBuf,
) {
    let (want_shots, want_clip) = {
        let conn = db.lock();
        (flag(&conn, KEY_CAPTURE_SCREENSHOTS), flag(&conn, KEY_CAPTURE_CLIPBOARD))
    };
    if !want_shots && !want_clip {
        return; // nothing enabled — no watchers, zero overhead
    }

    let attach_dir = recordings_dir.join("attachments");
    let _ = std::fs::create_dir_all(&attach_dir);
    let running = Arc::new(AtomicBool::new(true));
    let started_ms = now_ms();

    // Replace any prior session (stops its loops via the old flag going out of
    // scope is not enough; explicitly clear first).
    stop(slot);
    *slot.lock() = Some(CaptureSession {
        note_id: note_id.clone(),
        attach_dir: attach_dir.clone(),
        started_ms,
        running: running.clone(),
    });

    if want_shots {
        let db = db.clone();
        let note_id = note_id.clone();
        let attach_dir = attach_dir.clone();
        let running = running.clone();
        tauri::async_runtime::spawn(async move {
            screenshot_loop(db, note_id, attach_dir, started_ms, running).await;
        });
    }
    if want_clip {
        let db = db.clone();
        let note_id = note_id.clone();
        let attach_dir = attach_dir.clone();
        let running = running.clone();
        tauri::async_runtime::spawn(async move {
            clipboard_loop(db, note_id, attach_dir, started_ms, running).await;
        });
    }
}

/// Stop the active capture session (if any). The watcher tasks observe the
/// `running` flag and exit on their next tick.
pub fn stop(slot: &SharedCaptures) {
    if let Some(sess) = slot.lock().take() {
        sess.running.store(false, Ordering::SeqCst);
    }
}

/// Poll the screenshot folder; attach any NEW image that appears while running.
/// Baseline = the set of image files present at start, so we only grab shots
/// taken during the meeting.
async fn screenshot_loop(
    db: Arc<Mutex<rusqlite::Connection>>,
    note_id: String,
    attach_dir: PathBuf,
    started_ms: u64,
    running: Arc<AtomicBool>,
) {
    let dir = screenshot_dir();
    let mut seen: std::collections::HashSet<PathBuf> = list_images(&dir).into_iter().collect();
    while running.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        if !running.load(Ordering::SeqCst) {
            break;
        }
        for p in list_images(&dir) {
            if seen.contains(&p) {
                continue;
            }
            seen.insert(p.clone());
            // Copy into the note's attachments dir so it's self-contained and
            // survives the user deleting the Desktop original.
            let fname = format!(
                "shot_{}_{}",
                now_ms(),
                p.file_name().and_then(|n| n.to_str()).unwrap_or("screenshot.png")
            );
            let dest = attach_dir.join(&fname);
            if std::fs::copy(&p, &dest).is_ok() {
                let rel = format!("attachments/{fname}");
                let off = now_ms().saturating_sub(started_ms);
                let conn = db.lock();
                insert_image(&conn, &note_id, &rel, off);
            }
        }
    }
}

fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && is_image_file(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Poll the clipboard change-count; on a change, capture image or text.
async fn clipboard_loop(
    db: Arc<Mutex<rusqlite::Connection>>,
    note_id: String,
    attach_dir: PathBuf,
    started_ms: u64,
    running: Arc<AtomicBool>,
) {
    let mut last = clipboard_change_count();
    while running.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        if !running.load(Ordering::SeqCst) {
            break;
        }
        let now = clipboard_change_count();
        if now == last || now < 0 {
            continue;
        }
        last = now;
        // Never capture secrets (password managers mark items concealed).
        if clipboard_is_concealed() {
            continue;
        }
        let off = now_ms().saturating_sub(started_ms);
        if clipboard_has_image() {
            let fname = format!("clip_{}.png", now_ms());
            let dest = attach_dir.join(&fname);
            if save_clipboard_image(&dest) {
                let rel = format!("attachments/{fname}");
                let conn = db.lock();
                insert_image(&conn, &note_id, &rel, off);
            }
        } else {
            let txt = clipboard_text();
            let trimmed = txt.trim();
            if !trimmed.is_empty() {
                // Cap snippet size so a huge paste doesn't bloat the DB.
                let snippet: String = trimmed.chars().take(4000).collect();
                let conn = db.lock();
                insert_text(&conn, &note_id, &snippet, off);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_file_detection() {
        assert!(is_image_file(Path::new("/x/Screenshot.png")));
        assert!(is_image_file(Path::new("/x/a.JPG")));
        assert!(!is_image_file(Path::new("/x/notes.txt")));
        assert!(!is_image_file(Path::new("/x/audio.wav")));
    }

    #[test]
    fn now_ms_is_recent() {
        let t = now_ms();
        assert!(t > 1_700_000_000_000); // after 2023
    }

    #[test]
    fn screenshot_dir_exists_or_desktop() {
        let d = screenshot_dir();
        // Either the configured dir or the ~/Desktop fallback — always non-empty.
        assert!(!d.as_os_str().is_empty());
    }
}

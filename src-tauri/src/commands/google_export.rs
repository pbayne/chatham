//! Tauri commands for the Google-backed note actions:
//!   - `send_to_google_docs` — one-click export of a note to a formatted Doc.
//!   - `generate_followup_email` — draft an after-call email (action items,
//!     follow-ups, next steps) with the local LLM and stage it as a Gmail draft
//!     the user reviews before sending.
//!
//! Both reuse the single Google connection on `AppState.autopilot` (one consent
//! covers calendar + Docs + Gmail), and the follow-up generator reuses the same
//! LLM provider dispatch the summary feature uses — so it runs through the
//! user's local Ollama with no new configuration.

use super::err;
use super::summary::{resolve_provider, language_directive, note_language};
use crate::db;
use crate::{google_docs, google_gmail, AppState};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

/// The follow-up email prompt. Deliberately strict: extract only what was
/// actually agreed, attribute owners when named, and never invent commitments.
/// Output is plain text ready to paste/send — no markdown headings, since it
/// lands in an email body.
const FOLLOWUP_SYSTEM_PROMPT: &str = "\
You are drafting a concise, professional follow-up email after a meeting, from \
the point of view of the person who took the notes. Use the provided notes and \
transcript. Produce a ready-to-send plain-text email with these parts:\n\
1. A one or two sentence recap of the meeting's purpose and outcome.\n\
2. A section 'Action items:' as a plain list, one per line beginning with '- ', \
each naming the owner when the transcript makes it clear (e.g. '- Patrick: send \
the revised quote by Friday'). If no owner was named, omit the name.\n\
3. A section 'Next steps:' as a plain list of what happens next, with dates when \
they were stated.\n\
4. A brief sign-off.\n\
Rules: include ONLY commitments, decisions, and next steps that genuinely appear \
in the input. Do not invent action items, dates, or attendees. If there were no \
action items, say so plainly rather than padding. Keep it tight — this is an \
email, not minutes. Do not use markdown formatting or headings beyond the plain \
'Action items:' / 'Next steps:' labels.";

/// Result of a Docs export: the created document's URL.
#[derive(Serialize)]
pub struct DocsExportResult {
    pub url: String,
}

/// One-click: export a note to a new, formatted Google Doc. `include_summary`
/// and `include_transcript` mirror the file-export options.
#[tauri::command]
pub async fn send_to_google_docs(
    app: AppHandle,
    note_id: String,
    include_summary: bool,
    include_transcript: bool,
) -> Result<DocsExportResult, String> {
    let state: State<AppState> = app.state();

    let note = {
        let conn = state.db.lock();
        db::get_note(&conn, &note_id).map_err(err)?
    };
    let title = if note.title.trim().is_empty() {
        "Untitled note".to_string()
    } else {
        note.title.trim().to_string()
    };

    let blocks = google_docs::blocks_for_note(
        &title,
        &note.summary,
        &note.transcript,
        include_summary,
        include_transcript,
    );

    let (http, token) = {
        let http = state.autopilot.http();
        let token = state.autopilot.access_token().await.map_err(err)?;
        (http, token)
    };

    let url = google_docs::create_doc(&http, &token, &title, &blocks)
        .await
        .map_err(err)?;
    Ok(DocsExportResult { url })
}

/// Result of staging a follow-up: the draft body (so the UI can preview it) and
/// a link to Gmail Drafts.
#[derive(Serialize)]
pub struct FollowupResult {
    pub draft_id: String,
    pub gmail_url: String,
    pub subject: String,
    pub body: String,
    pub recipients: Vec<String>,
}

/// Generate an after-call follow-up email with the local LLM and stage it as a
/// Gmail draft. Recipients are prefilled from the note's calendar attendees
/// (when Autopilot created it); the user reviews and sends from Gmail.
#[tauri::command]
pub async fn generate_followup_email(
    app: AppHandle,
    note_id: String,
) -> Result<FollowupResult, String> {
    let state: State<AppState> = app.state();

    // Resolve the LLM provider the same way summaries do (local Ollama etc.).
    let provider_id = {
        let conn = state.db.lock();
        let n = db::get_note(&conn, &note_id).map_err(err)?;
        super::summary::summary_provider_id(&conn, &n).map_err(|e| e.to_string())?
    };
    let api_key = match provider_id.keychain_account() {
        Some(_) => super::read_provider_api_key(&state, provider_id.as_str())?,
        None => None,
    };

    let (provider, language, note) = {
        let conn = state.db.lock();
        let n = db::get_note(&conn, &note_id).map_err(err)?;
        let p = resolve_provider(&conn, &n, api_key).map_err(|e| e.to_string())?;
        let lang = note_language(&conn, &n).map_err(|e| e.to_string())?;
        (p, lang, n)
    };

    if note.transcript.trim().is_empty() && note.body.trim().is_empty() {
        return Err("This note has no transcript or notes to draft from.".to_string());
    }

    // Build the LLM input from notes + transcript (same shape as summaries).
    let body_text = crate::html_text::html_to_text(&note.body);
    let notes_block = if body_text.trim().is_empty() {
        "[Notes]\n(none)".to_string()
    } else {
        format!("[Notes]\n{body_text}")
    };
    let transcript_block = if note.transcript.trim().is_empty() {
        "[Transcript]\n(none)".to_string()
    } else {
        format!("[Transcript]\n{}", note.transcript)
    };

    // Structured, already-extracted action items (Phase A: owner + due) and the
    // note's client, pulled from the DB so the draft agrees with the Follow-ups
    // view rather than re-deriving items. Best-effort: a read failure just omits
    // the block and the model falls back to the transcript.
    let (items_block, client_name) = {
        let conn = state.db.lock();
        let items = db::list_action_items(&conn, &note_id)
            .map(|v| {
                v.into_iter()
                    .map(|a| (a.text, a.owner, a.due, a.done))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let client = note
            .client_id
            .as_deref()
            .and_then(|id| db::client_name(&conn, id).ok().flatten());
        (action_items_block(&items), client)
    };

    let mut user_message = format!("{notes_block}\n\n{transcript_block}");
    if let Some(ref client) = client_name {
        user_message.push_str(&format!("\n\n[Client]\n{client}"));
    }
    if let Some(ref items) = items_block {
        user_message.push_str(&format!(
            "\n\n[Confirmed action items — use THESE verbatim for the Action items section; do not invent others]\n{items}"
        ));
    }
    let full_prompt = format!("{FOLLOWUP_SYSTEM_PROMPT}\n\n{}", language_directive(&language));

    // No streaming surface for this action — a no-op chunk sink.
    let body = provider
        .complete(&full_prompt, &user_message, |_| {})
        .await
        .map_err(|e| e.to_string())?;

    // Recipients from calendar attendees, if this note came from Autopilot.
    let recipients = {
        let conn = state.db.lock();
        recipients_from_meta(&conn, &note_id)
    };

    let subject = format!("Follow-up: {}", title_or_default(&note.title));

    let spec = google_gmail::DraftSpec {
        to: recipients.clone(),
        subject: subject.clone(),
        body: body.clone(),
    };

    let (http, token) = {
        let http = state.autopilot.http();
        let token = state.autopilot.access_token().await.map_err(err)?;
        (http, token)
    };
    let auth = crate::gmail_auth::GmailAuth {
        access_token: token,
        extra_headers: vec![],
    };
    let (draft_id, gmail_url) = google_gmail::create_draft(&http, &auth, &spec)
        .await
        .map_err(err)?;

    Ok(FollowupResult {
        draft_id,
        gmail_url,
        subject,
        body,
        recipients,
    })
}

fn title_or_default(title: &str) -> String {
    if title.trim().is_empty() {
        "Meeting".to_string()
    } else {
        title.trim().to_string()
    }
}

/// Format the note's stored (structured) action items into a prompt block the
/// follow-up draft treats as ground truth, so the email agrees with the
/// Follow-ups view instead of the model re-deriving items from the transcript.
/// Only OPEN items (not done) are included — a follow-up is about what's still
/// outstanding. Each line carries owner and due when known. Returns None when
/// there are no open items, so the caller can tell the model to fall back to
/// reading the transcript. Pure + unit-testable (no DB).
fn action_items_block(items: &[(String, String, String, bool)]) -> Option<String> {
    // items: (text, owner, due, done)
    let mut lines = Vec::new();
    for (text, owner, due, done) in items {
        if *done || text.trim().is_empty() {
            continue;
        }
        let mut line = String::from("- ");
        if !owner.trim().is_empty() {
            line.push_str(owner.trim());
            line.push_str(": ");
        }
        line.push_str(text.trim());
        if !due.trim().is_empty() {
            line.push_str(" (due ");
            line.push_str(due.trim());
            line.push(')');
        }
        lines.push(line);
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Pull attendee emails from a note's calendar metadata (excluding the user
/// themselves — they don't email themselves a follow-up). Best-effort: returns
/// empty if the note wasn't Autopilot-created or the JSON is unreadable.
fn recipients_from_meta(conn: &rusqlite::Connection, note_id: &str) -> Vec<String> {
    let Ok(Some(meta)) = db::get_note_calendar_meta(conn, note_id) else {
        return Vec::new();
    };
    recipients_from_attendees_json(&meta.attendees_json)
}

/// The pure part of `recipients_from_meta`: given the stored attendees JSON
/// (an array of `google_calendar::Attendee`), return the email addresses to put
/// on the follow-up, dropping the user themselves and any blank addresses.
fn recipients_from_attendees_json(attendees_json: &str) -> Vec<String> {
    let Ok(attendees) = serde_json::from_str::<Vec<serde_json::Value>>(attendees_json) else {
        return Vec::new();
    };
    attendees
        .iter()
        .filter(|a| !a.get("is_self").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|a| a.get("email").and_then(|v| v.as_str()).map(str::to_string))
        .filter(|e| !e.trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipients_exclude_self_and_blanks() {
        // Shape matches google_calendar::Attendee serialization.
        let json = r#"[
            {"email":"patrick.bayne@databricks.com","display_name":"Patrick","response_status":"accepted","organizer":false,"is_self":true},
            {"email":"brad@databricks.com","display_name":"Bradley","response_status":"accepted","organizer":true,"is_self":false},
            {"email":"","display_name":"No Email","response_status":"needsAction","organizer":false,"is_self":false},
            {"email":"someone@x.com","display_name":"Someone","response_status":"tentative","organizer":false,"is_self":false}
        ]"#;
        let got = recipients_from_attendees_json(json);
        assert_eq!(got, vec!["brad@databricks.com", "someone@x.com"]);
    }

    #[test]
    fn recipients_empty_on_bad_json() {
        assert!(recipients_from_attendees_json("not json").is_empty());
        assert!(recipients_from_attendees_json("[]").is_empty());
    }

    #[test]
    fn recipients_missing_is_self_defaults_to_included() {
        // An attendee object without is_self is treated as not-self (included).
        let json = r#"[{"email":"a@b.com","display_name":"A"}]"#;
        assert_eq!(recipients_from_attendees_json(json), vec!["a@b.com"]);
    }

    fn item(text: &str, owner: &str, due: &str, done: bool) -> (String, String, String, bool) {
        (text.into(), owner.into(), due.into(), done)
    }

    #[test]
    fn action_items_block_formats_owner_and_due() {
        let items = vec![
            item("Send the SOW", "Sarah", "2026-03-13", false),
            item("Book the follow-up", "", "", false),
        ];
        let got = action_items_block(&items).unwrap();
        assert_eq!(
            got,
            "- Sarah: Send the SOW (due 2026-03-13)\n- Book the follow-up"
        );
    }

    #[test]
    fn action_items_block_skips_done_and_blank() {
        let items = vec![
            item("Already handled", "Pat", "", true),
            item("   ", "", "", false),
        ];
        assert!(action_items_block(&items).is_none());
    }

    #[test]
    fn action_items_block_none_when_empty() {
        assert!(action_items_block(&[]).is_none());
    }
}

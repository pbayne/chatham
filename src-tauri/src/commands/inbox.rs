//! Tauri commands for Inbox Triage — the "turn my inbox into a to-do board"
//! feature. Reuses the single Google connection on `AppState.autopilot`
//! (one consent covers calendar + Docs + Gmail compose + Gmail readonly) and
//! the same LLM dispatch summaries use, defaulting to the user's local Ollama
//! so triage is free and private out of the box.
//!
//! Flow per triage run:
//!   1. list unread INBOX messages (gmail.readonly)
//!   2. skip any message id already triaged (SQLite idempotency, like a
//!      processed-message store — re-running never double-processes)
//!   3. for each new message: fetch body, classify + extract action items with
//!      the LLM, persist the verdict
//!   4. the UI shows the board; the user approves a card to stage a Gmail DRAFT
//!      (gmail.compose, never auto-sent — the user reviews and sends it).

use super::err;
use super::read_provider_api_key;
use crate::db;
use crate::gmail_read::{self, InboxMessage};
use crate::triage::{self, Category, Verdict};
use crate::{anthropic, google_gmail, openai, AppState};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

/// Ollama's default OpenAI-compat endpoint — same constant the summary path
/// uses. A user on LM Studio / llama-server / vLLM overrides it in Settings.
const DEFAULT_LOCAL_LLM_BASE_URL: &str = "http://localhost:11434/v1";
/// How many unread messages one run will triage. Keeps a first run on a big
/// backlog bounded (and a local model's latency sane).
const MAX_PER_RUN: u32 = 25;

/// A triaged card for the board: the email plus its verdict.
#[derive(Serialize)]
pub struct TriageCard {
    pub message: InboxMessage,
    pub category: String,
    pub reason: String,
    pub action_items: Vec<String>,
    pub suggested_reply: String,
    /// Set once the user has approved and a Gmail draft exists for this card.
    pub draft_id: Option<String>,
}

/// Status for the Inbox panel header.
#[derive(Serialize)]
pub struct InboxStatus {
    pub connected: bool,
    pub provider: String,
    pub triaged_count: i64,
}

// ─── SQLite state (idempotency + the board's backing store) ──────────────────

/// Create the triage table if it doesn't exist. Called at the top of every
/// command so there's no migration to coordinate for this feature branch.
fn ensure_table(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS inbox_triage (
            message_id   TEXT PRIMARY KEY,
            thread_id    TEXT NOT NULL DEFAULT '',
            from_addr    TEXT NOT NULL DEFAULT '',
            subject      TEXT NOT NULL DEFAULT '',
            snippet      TEXT NOT NULL DEFAULT '',
            category     TEXT NOT NULL DEFAULT 'fyi',
            reason       TEXT NOT NULL DEFAULT '',
            action_items TEXT NOT NULL DEFAULT '[]',
            reply        TEXT NOT NULL DEFAULT '',
            draft_id     TEXT,
            triaged_at   INTEGER NOT NULL DEFAULT 0
        );",
    )?;
    Ok(())
}

fn already_triaged(conn: &rusqlite::Connection, message_id: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM inbox_triage WHERE message_id = ?1",
        [message_id],
        |_| Ok(()),
    )
    .is_ok()
}

fn save_verdict(
    conn: &rusqlite::Connection,
    msg: &InboxMessage,
    v: &Verdict,
) -> anyhow::Result<()> {
    let action_items = serde_json::to_string(&v.action_items).unwrap_or_else(|_| "[]".into());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO inbox_triage
           (message_id, thread_id, from_addr, subject, snippet, category, reason, action_items, reply, draft_id, triaged_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,?10)
         ON CONFLICT(message_id) DO UPDATE SET
           category=excluded.category, reason=excluded.reason,
           action_items=excluded.action_items, reply=excluded.reply,
           triaged_at=excluded.triaged_at",
        rusqlite::params![
            msg.id,
            msg.thread_id,
            msg.from,
            msg.subject,
            msg.snippet,
            v.category.as_str(),
            v.reason,
            action_items,
            v.suggested_reply,
            now,
        ],
    )?;
    Ok(())
}

// ─── LLM dispatch (note-free; defaults to local Ollama) ──────────────────────

/// Resolve which provider triage uses: the `triage_provider` setting, default
/// `local`. Returns (provider_id_str, base_url, model, think, api_key).
fn resolve_triage_provider(
    state: &State<AppState>,
) -> Result<(String, String, String, bool, String), String> {
    let conn = state.db.lock();
    let provider = db::get_setting(&conn, "triage_provider")
        .ok()
        .flatten()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".into());
    drop(conn);

    match provider.as_str() {
        "openai" => {
            let api_key = read_provider_api_key(state, "openai")?
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "OpenAI API key not set — add it in Settings or use the local model".to_string())?;
            let conn = state.db.lock();
            let model = db::get_setting(&conn, "summary_model")
                .ok()
                .flatten()
                .unwrap_or_else(|| "gpt-4o-mini".into());
            Ok(("openai".into(), openai::BASE.into(), model, false, api_key))
        }
        "anthropic" => {
            let api_key = read_provider_api_key(state, "anthropic")?
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Anthropic API key not set".to_string())?;
            let conn = state.db.lock();
            let model = db::get_setting(&conn, "anthropic_model")
                .ok()
                .flatten()
                .unwrap_or_else(|| anthropic::DEFAULT_MODEL.to_string());
            Ok(("anthropic".into(), anthropic::BASE.into(), model, false, api_key))
        }
        _ => {
            // local (Ollama / any OpenAI-compatible server)
            let conn = state.db.lock();
            let base_url = db::get_setting(&conn, "local_llm_base_url")
                .ok()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| DEFAULT_LOCAL_LLM_BASE_URL.to_string());
            let model = db::get_setting(&conn, "local_llm_model")
                .ok()
                .flatten()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    "Local LLM model not configured — pick one in Settings, or switch triage to OpenAI".to_string()
                })?;
            let think = db::get_setting(&conn, "local_llm_think")
                .ok()
                .flatten()
                .map(|s| s.trim().eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            Ok(("local".into(), base_url, model, think, "humla-local".into()))
        }
    }
}

/// One classification call. No streaming surface — a no-op chunk sink.
async fn classify(
    provider: &str,
    base_url: &str,
    api_key: &str,
    model: &str,
    think: bool,
    msg: &InboxMessage,
) -> anyhow::Result<Verdict> {
    let user_message = triage::build_user_message(msg);
    let raw = if provider == "anthropic" {
        anthropic::summarize(
            api_key,
            model,
            triage::TRIAGE_SYSTEM_PROMPT,
            &user_message,
            |_| {},
        )
        .await?
    } else {
        openai::summarize_with_base(
            base_url,
            api_key,
            model,
            think,
            triage::TRIAGE_SYSTEM_PROMPT,
            &user_message,
            |_| {},
        )
        .await?
    };
    Ok(triage::parse_verdict(&raw))
}

// ─── Commands ────────────────────────────────────────────────────────────────

/// Status for the Inbox panel.
#[tauri::command]
pub fn inbox_status(state: State<AppState>) -> Result<InboxStatus, String> {
    let conn = state.db.lock();
    ensure_table(&conn).map_err(err)?;
    let triaged_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM inbox_triage", [], |r| r.get(0))
        .unwrap_or(0);
    let provider = db::get_setting(&conn, "triage_provider")
        .ok()
        .flatten()
        .unwrap_or_else(|| "local".into());
    Ok(InboxStatus {
        connected: state.autopilot.is_connected(),
        provider,
        triaged_count,
    })
}

/// Triage the inbox now: fetch unread, classify the new ones, persist verdicts.
/// Returns how many new messages were triaged this run.
#[tauri::command]
pub async fn inbox_triage_now(app: AppHandle) -> Result<usize, String> {
    let state: State<AppState> = app.state();
    {
        let conn = state.db.lock();
        ensure_table(&conn).map_err(err)?;
    }
    if !state.autopilot.is_connected() {
        return Err("Not connected to Google — connect in Settings first.".to_string());
    }

    let (provider, base_url, model, think, api_key) = resolve_triage_provider(&state)?;

    let (http, token) = {
        let http = state.autopilot.http();
        let token = state.autopilot.access_token().await.map_err(err)?;
        (http, token)
    };

    let ids = gmail_read::list_unread(&http, &token, MAX_PER_RUN)
        .await
        .map_err(err)?;

    let mut triaged = 0usize;
    for id in ids {
        {
            let conn = state.db.lock();
            if already_triaged(&conn, &id) {
                continue;
            }
        }
        let msg = match gmail_read::fetch_message(&http, &token, &id).await {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[inbox] fetch {id} failed: {e}");
                continue;
            }
        };
        let verdict = match classify(&provider, &base_url, &api_key, &model, think, &msg).await {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[inbox] classify {id} failed: {e}");
                continue;
            }
        };
        {
            let conn = state.db.lock();
            if let Err(e) = save_verdict(&conn, &msg, &verdict) {
                eprintln!("[inbox] save {id} failed: {e}");
                continue;
            }
        }
        triaged += 1;
    }
    Ok(triaged)
}

/// List triaged cards for the board, newest first. Optional category filter.
#[tauri::command]
pub fn inbox_list(
    state: State<AppState>,
    category: Option<String>,
) -> Result<Vec<TriageCard>, String> {
    let conn = state.db.lock();
    ensure_table(&conn).map_err(err)?;

    let cat_filter = category
        .as_deref()
        .map(|c| Category::parse_loose(c).as_str().to_string());

    let sql = "SELECT message_id, thread_id, from_addr, subject, snippet, category, reason, action_items, reply, draft_id
               FROM inbox_triage";
    let mut cards = Vec::new();
    let mut stmt = conn
        .prepare(&format!("{sql} ORDER BY triaged_at DESC"))
        .map_err(err)?;
    let rows = stmt
        .query_map([], |r| {
            let action_items_json: String = r.get(7)?;
            let action_items: Vec<String> =
                serde_json::from_str(&action_items_json).unwrap_or_default();
            Ok(TriageCard {
                message: InboxMessage {
                    id: r.get(0)?,
                    thread_id: r.get(1)?,
                    from: r.get(2)?,
                    to: String::new(),
                    subject: r.get(3)?,
                    date: String::new(),
                    snippet: r.get(4)?,
                    body: String::new(),
                },
                category: r.get(5)?,
                reason: r.get(6)?,
                action_items,
                suggested_reply: r.get(8)?,
                draft_id: r.get(9)?,
            })
        })
        .map_err(err)?;
    for row in rows {
        let card = row.map_err(err)?;
        if let Some(ref want) = cat_filter {
            if &card.category != want {
                continue;
            }
        }
        cards.push(card);
    }
    Ok(cards)
}

/// Approve a card: stage its (possibly user-edited) reply as a Gmail draft to
/// the message's sender. Never auto-sends — lands in Drafts for review.
#[tauri::command]
pub async fn inbox_approve_draft(
    app: AppHandle,
    message_id: String,
    to: String,
    subject: String,
    body: String,
) -> Result<String, String> {
    let state: State<AppState> = app.state();
    let spec = google_gmail::DraftSpec {
        to: to
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        subject,
        body,
    };
    let (http, token) = {
        let http = state.autopilot.http();
        let token = state.autopilot.access_token().await.map_err(err)?;
        (http, token)
    };
    let (draft_id, gmail_url) = google_gmail::create_draft(&http, &token, &spec)
        .await
        .map_err(err)?;
    {
        let conn = state.db.lock();
        let _ = conn.execute(
            "UPDATE inbox_triage SET draft_id = ?1 WHERE message_id = ?2",
            rusqlite::params![draft_id, message_id],
        );
    }
    Ok(gmail_url)
}

/// Set the triage LLM provider (local / openai / anthropic).
#[tauri::command]
pub fn inbox_set_provider(state: State<AppState>, provider: String) -> Result<String, String> {
    let normalized = match provider.trim().to_lowercase().as_str() {
        "openai" => "openai",
        "anthropic" => "anthropic",
        _ => "local",
    }
    .to_string();
    let conn = state.db.lock();
    db::set_setting(&conn, "triage_provider", &normalized).map_err(err)?;
    Ok(normalized)
}

/// Clear a single card (e.g. dismiss Noise) or all cards.
#[tauri::command]
pub fn inbox_clear(state: State<AppState>, message_id: Option<String>) -> Result<(), String> {
    let conn = state.db.lock();
    ensure_table(&conn).map_err(err)?;
    match message_id {
        Some(id) => {
            conn.execute("DELETE FROM inbox_triage WHERE message_id = ?1", [id])
                .map_err(err)?;
        }
        None => {
            conn.execute("DELETE FROM inbox_triage", []).map_err(err)?;
        }
    }
    Ok(())
}

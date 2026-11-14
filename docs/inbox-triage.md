# Inbox Triage — how to run it

An AI inbox-triage mode built into Humla. It reads your unread Gmail, sorts it
into Action Needed / Follow-ups / FYI / Noise, extracts to-dos, and drafts
replies you approve before sending. Runs on a local model by default (free,
private). Branch: `feat/inbox-triage`.

## Run it

```bash
cd ~/Development/humla
npm run tauri dev          # or install the built .app
```

Then: click **Inbox** in the left sidebar → **Triage now**.

## The two header controls

- **Provider** — which model classifies/drafts:
  - `Local (free)` — your Ollama model (default; set via Settings, e.g. phi3).
    Free + private, modest quality on small models.
  - `OpenAI` / `Anthropic` — sharper results, uses your API key, costs tokens.
- **Auth** — how Humla reaches Gmail:
  - `OAuth` — Humla's own Google OAuth. Needs a real client ID compiled in
    (`GOOGLE_OAUTH_CLIENT_ID`) AND an account allowed to authorize third-party
    apps. **Does NOT work on a @databricks.com account** (admin policy blocks
    third-party OAuth — go/accessblock).
  - `gcloud` — uses the machine's existing Google Cloud Application Default
    Credentials (`gcloud auth application-default print-access-token`) + the
    quota-project header. **This is the mode that works on a corp account**: no
    OAuth client, no consent screen, no policy exception. Requires
    `gcloud auth application-default login` to have been run (it has, here).

## Requirements

- For `Local` provider: Ollama running with a model (`ollama serve` + the model
  named in Settings → local_llm_model).
- For `gcloud` auth: gcloud SDK installed + ADC logged in (gmail.modify scope).

## What each card shows

- Subject + sender, and a one-line reason for the bucket.
- **To do**: extracted action items (owner-attributed when the email names one).
- **Draft reply** (Action Needed / Follow-ups only): an editable reply.
  **Approve → draft** stages it in Gmail **Drafts** — it is NEVER auto-sent.
- **Dismiss** removes the card. **Clear** (header) empties the board so a
  re-triage starts fresh.

## Notes / limits

- Idempotent: re-running "Triage now" never re-processes a message already on
  the board (SQLite state in `inbox_triage` table).
- First run is capped at 25 unread messages per run (MAX_PER_RUN in
  commands/inbox.rs) to keep a big backlog + local-model latency bounded.
- Giving this to a colleague: they need either their own gcloud ADC (corp
  account) or a personal Google OAuth client ID for OAuth mode. For ≤100 users
  on personal Gmail, OAuth "Testing" mode needs no Google verification.

## Code map

- `src-tauri/src/gmail_read.rs` — list unread + MIME body extraction (pure, tested)
- `src-tauri/src/triage.rs` — classify + extract engine (pure, tested)
- `src-tauri/src/gmail_auth.rs` — oauth vs gcloud token resolution (tested)
- `src-tauri/src/commands/inbox.rs` — orchestrator + Tauri commands
- `src/pages/Inbox.tsx` — the board UI

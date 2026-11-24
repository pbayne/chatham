# Roadmap: meeting intelligence features (1–4)

Status: DESIGN + incremental build. Branch `spike/turso-local-store`.
Four features the user asked for, sequenced by dependency and leverage. All
build on infrastructure that ALREADY exists (voiceprints, client tags,
`note_action_items` with owner+due columns, `generate_followup_email`,
FTS5 chunks) — none is greenfield.

## Ground-truth findings (what already exists — don't rebuild)
- `note_action_items` table ALREADY has `owner` and `due` columns (db.rs ~427).
  The auto-extract (`action_items.rs`) captures `owner` but HARDCODES `due=""`
  (line 190) and the LLM struct `ExtractedItem` has no `due` field. So due-date
  capture is a prompt+struct change, NOT a migration.
- `list_open_action_items(workspace)` already powers a global Follow-ups view.
- `generate_followup_email` drafts an after-call email from the transcript but
  does NOT yet pull the structured action items or client context.
- Voiceprints enroll names; harvest attaches company; notes carry `client_id`.
  There is NO cross-note aggregation view keyed by person or client yet.

## Sequence (dependency-ordered)

### Phase A — Feature 2: speaker-aware action items with owner + due (SMALLEST, FIRST)  ✅ DONE
Self-contained backend + prompt change. No schema migration.
- Add `due: String` to `ExtractedItem`; update `extract_prompt` to ask for
  `owner` (the speaker who committed) and `due` (ISO date if a deadline is
  stated, else ""). Keep "don't invent" guard.
- Stop hardcoding due="" at the create call — pass `it.due`.
- Normalize relative dates ("Friday", "next week") → ISO against the note's
  date, in pure testable Rust (don't make the LLM do date math).
- Tests: parse owner+due; relative-date normalization; empty due tolerated.
- Payoff: Follow-ups view (already built) now shows who-owes-what-by-when.

### Phase B — Feature 4: smart follow-up email (depends on A)  ✅ DONE
Upgrade `generate_followup_email` to include the structured action items
(owner + due) and the note's client, so the draft is "here's what we agreed,
who owns what, by when" instead of a generic recap. Still a Gmail DRAFT, never
auto-sent (Patrick's rule). Mostly prompt + input assembly; recipients already
come from calendar attendees.

### Phase C — shared aggregation layer (foundation for 1 & 3)  ✅ CORE DONE (pure engine + tests)
A new read-only module querying across notes:
- by PERSON (voiceprint name / harvested name appearing as a speaker label),
- by CLIENT (`client_id`).
Returns: notes they appear in, date range, their action items, recurring
topics (FTS/keyword), last-seen. Pure SQL + aggregation over existing tables.
This is the one genuinely new building block; 1 and 3 are views on top.

### Phase D — Feature 1: cross-note person intelligence (view on C)  ⬜ TODO (deferred)
A "person" view: everywhere <name> spoke, their commitments, recurring themes,
last seen. Surfaced from the speaker label / voiceprint roster.
NEXT STEPS when resumed:
- DB wrappers over `db::list_notes_filtered` (already supports `speaker` +
  `speaker_alias` for the "You" sentinel) → feed `intelligence::person_activity`.
- Per-note open items via `db::list_action_items` filtered to owner == name.
- Tauri command `person_activity(name)` returning `PersonActivity`.
- New UI view + an entry point from the speaker roster / a speaker pill.

### Phase E — Feature 3: per-client "what changed since last call" digest (view on C)  ⬜ TODO (deferred)
A rolling client digest: decisions, open questions, action items still open,
what moved since the previous call with that client. LLM-summarized over the
client's recent notes (bounded char budget, like the other passes).
NEXT STEPS when resumed:
- DB wrapper: `list_notes_filtered` with `client_id` → `intelligence::client_activity`.
- Tauri command `client_activity(client_id)` returning `ClientActivity`.
- LLM digest pass over the client's recent note summaries (reuse the summary
  provider dispatch; bounded char budget like action-item extraction).
- New UI view, entry point from the client tag / client filter.

> STATUS (2026-11): Phases A, B, C-core are DONE + tested + uncommitted on
> branch `spike/turso-local-store`. D and E are deferred to a later session as
> "other augmentations." The `intelligence` module's `client_activity` /
> `person_activity` compile with dead-code warnings until D/E wire them — that
> is expected, not a regression.

## Principles
- Each phase ships independently green (typecheck + tests) and uncommitted for
  Patrick's visual review before the next.
- No new sync-wire fields without need: action items are already local-only
  (ADR pattern); the aggregation layer is read-only derived data.
- Date math and name/company parsing stay in pure, unit-tested functions, not
  the LLM (the LLM proposes; deterministic code normalizes).
- UI changes to Note.tsx / new views are high-craft: show rendered before
  landing, verify across themes.

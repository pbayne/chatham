# Spike: Postgres-backed sync for Chatham (Neon/Lakebase cloud + PGlite local mirror)

Status: SCOPING. Branch `spike/turso-local-store` (reuse; rename later if this
becomes real — the Turso doc in this dir is superseded by THIS one for the
team-backend goal).
Author: scoping pass, 2026-11.

## Goal (what the user actually wants)
A cloud-hosted sync/collaboration backend that teams can SHARE, with **Postgres
as the long-term foundation** so it can run on **Neon or Lakebase** in the cloud.
**PGlite** is the LOCAL DEV/TEST MIRROR (real Postgres in 3MB, embedded) so the
same client code runs against a tiny local Postgres in dev/CI and against
Neon/Lakebase in prod with no dialect difference.

## Critical framing (the two dead ends we ruled out, don't revisit)
- **Turso** does NOT bridge Chatham to Neon/Lakebase. Turso "speaking Postgres"
  means Turso ITSELF accepts PG connections; it cannot act as a client TO another
  Postgres. Irrelevant to this goal. (See turso-local-store-spike.md.)
- **PGlite is NOT a team backend.** It runs Postgres in single-user mode = ONE
  connection; `pglite-socket` only multiplexes over that single conn (writes
  serialize, "not all cases covered"). Electric's own guidance: "Multiple app
  servers sharing one database: use a Postgres server." So PGlite = local mirror
  ONLY. The SHARED cloud backend must be a real PG server (Neon / Lakebase /
  plain Postgres).
- Zero-rewrite alternative that was explicitly passed over: cloud-host
  PocketBase (keeps today's client). We are NOT doing that — this doc commits to
  the Postgres rewrite for the long-term foundation.

## What this actually is: a rewrite of the sync CLIENT, not a backend swap
Chatham's client speaks the **PocketBase REST protocol** (collections CRUD,
`auth-with-password`, `/api/files`, custom `/api/chatham/*` hooks). Neon/Lakebase/
PGlite are **bare SQL (Postgres wire)** — none of that REST/auth/file/hooks layer
exists. So moving to Postgres = reimplementing everything the client currently
gets for free from PocketBase.

### Measured surface (the work)
- `src-tauri/crates/cloud-sync/src/lib.rs` — **3224 lines**, the sync engine
  (notes/folders/clients/summary_prompts/note_sessions/note_revocations
  push+pull+outbox+LWW). ~40 rusqlite sites + the PocketBase REST calls.
- `src-tauri/src/commands/cloud.rs` — **2310 lines** (auth, workspaces,
  membership, billing, file-asset sync).
- `src-tauri/src/commands/cloud_auth.rs` — **1257 lines** (login, 2FA, verify,
  password reset).
- `src-tauri/src/commands/chat.rs` — **3694 lines** (cloud chat; server-brokered).
- `src-tauri/src/chat/cloud.rs` — **904 lines**.
- **Total ~11,400 lines** in the cloud/sync layer.
- **109** PocketBase-protocol references, **177** reqwest HTTP verb call sites
  across these files.

### The hard architectural truth: NO transport seam exists
The only trait is `Session` (cloud-sync/lib.rs:61) and it abstracts ONLY token
provision, not transport. Every PocketBase HTTP call is inline. There is no
`trait Backend`/`trait Transport` to swap a Postgres impl behind. So either:
  (a) INTRODUCE that seam first (refactor the PocketBase calls behind a
      `trait SyncBackend`), then write a second impl — large but clean, keeps
      PocketBase working during migration; OR
  (b) fork the sync layer and rewrite in place against Postgres — faster to a
      prototype, but no fallback and a huge diff.

### What the Postgres backend must provide that PocketBase gave for free
These are the REAL line items (PocketBase bundled them; Postgres doesn't):
1. **Auth** — password login, token issue/refresh, email verification, password
   reset, 2FA. (PocketBase's `users` collection + auth routes.) On Postgres this
   is a service you must build/host, NOT a SQL feature. → likely a thin auth
   service in front of PG, or adopt an auth provider. BIGGEST hidden cost.
2. **File storage** — note `audio`, session `playback/mic/sys/timeline/chunks`.
   PocketBase handled protected file fields + `/api/files/token`. Postgres is not
   a blob store → need S3/R2/volume + a signed-URL scheme.
3. **The 9 collections** become real PG tables (schema already reverse-engineered
   in `SELF_HOST_SCHEMA.md` — reusable as the DDL starting point). The
   client-authoritative LWW model (`client_id`, `workspace`, `client_updated_at`,
   `deleted` tombstone, `updated` pull cursor) maps cleanly to PG columns +
   indexes.
4. **Custom hooks** (`find-user`, invite/leave/transfer, chat brokering) → become
   API endpoints in whatever service fronts PG, or SQL functions.
5. **Row access rules** (PocketBase `listRule` etc.) → enforced in the service
   layer or PG RLS.

### Decision that shapes everything: does the client talk SQL directly, or via an API?
- **Option A — client → Postgres directly (wire protocol).** Rust client uses
  `tokio-postgres`/`sqlx` straight to Neon/Lakebase. Simplest data path, BUT
  every device needs DB credentials + the auth/file/rules layers still have to
  live somewhere (can't put auth in the DB). And exposing Neon/Lakebase directly
  to end-user devices is a security smell.
- **Option B — client → thin API service → Postgres.** Reintroduces a server
  tier (the thing PocketBase WAS). More infra, but proper auth/files/rules and
  you don't ship DB creds to clients. This is the honest shape of a real
  multi-tenant team backend.
→ RECOMMENDATION: Option B. At which point, reconsider whether a cloud PocketBase
  (which already IS option B, on SQLite) is truly worse than building the same
  tier on Postgres. The ONLY durable reason to prefer PG here is a hard
  requirement for Lakebase/Neon specifically (governance, Databricks
  integration, existing PG ops). Name that requirement before committing.

## PGlite's concrete, correct role here
- Local dev DB + CI: spin a fresh PGlite per test, run the real DDL from
  SELF_HOST_SCHEMA-derived migrations, exercise the sync queries against REAL
  Postgres — 3MB, instant. Excellent and low-risk.
- Dialect parity gate: anything that passes against PGlite will run on
  Neon/Lakebase (same Postgres). Catches SQLite-isms early.
- NOT the server. Never the shared backend.

## Recommended de-risking path (cheap proofs before the 11k-line rewrite)
1. **Decide A vs B and name the Lakebase/Neon hard requirement.** If none, STOP
   and cloud-host PocketBase instead — far less work for the same team outcome.
2. If committing to PG: **introduce `trait SyncBackend`** in cloud-sync behind
   the existing PocketBase calls (pure refactor, PocketBase still the only impl,
   ships green). This creates the seam that today does not exist — do this FIRST;
   it's valuable even if the PG impl is deferred.
3. **Stand up the schema on PGlite** from SELF_HOST_SCHEMA.md DDL; port the
   sync engine's read/write queries to a PG `SyncBackend` impl; run the existing
   sync tests (`FakeSession` at lib.rs:3016 shows the test seam) against PGlite.
4. Only then tackle auth + file storage (the real cost centers) and point the PG
   impl at Neon/Lakebase.

## Backend candidates vs the REAL requirements (efficient storage + encryption + team RBAC/ABAC)
The goal was never "which SQL engine" — it's "what replaces PocketBase" given
three needs: efficient storage, encryption, team permissions (RBAC/ABAC). All
candidates are BaaS platforms with their OWN protocol, so ANY of them is still a
sync-client rewrite. Ranked by fit:

- **Appwrite — best match to the exact three needs; closest sibling to PocketBase.**
  - RBAC/ABAC: NATIVE. `Role.team(id)`, `Role.team(id,"owner")`, `Role.user`,
    `Role.label`; collection- AND document-level rules; Teams are a built-in
    primitive; cross-tenant access returns 401 from the engine, not an app-code
    filter. This is the team model we want and PocketBase's weakest area.
  - Encryption: per-file storage encryption (AES-256-GCM, per-file key wrapped by
    server key, segmented streaming); encrypted text DB columns (AES-128-GCM) —
    CAVEAT: encrypted columns can't be queried. Self-host: set `_APP_OPENSSL_KEY_V1`.
  - Storage: document DB + buckets w/ compression + antivirus. Self-hostable (Docker).
  - Migration cost: REST shape similar to PocketBase (collections+teams) → SHORTEST
    rewrite of the real options.
- **Supabase — most powerful/flexible; Postgres-native (the Neon/Lakebase tie-in).**
  - RBAC/ABAC: Postgres RLS — arbitrary SQL policies, ABAC = any WHERE clause
    incl. JWT team claims. More powerful than Appwrite's declarative roles, but
    YOU author/own every policy (open-table footgun on self-host).
  - Encryption: AES-256 at rest + TLS; app-layer encryption of secrets via Vault.
    Disk/column-level, NOT Appwrite's per-file E2E storage model.
  - Storage: Postgres + S3-backed, RLS-governed. Self-host heavier (PostgREST +
    GoTrue + Storage + Kong + Studio).
- **Turso — NO built-in auth/RBAC/teams/file layer.** Pitch is db-per-tenant +
  encryption + replication; you'd build permissions yourself. Fails the RBAC
  requirement out of the box. Not recommended for this goal.
- **PGlite — local dev/CI mirror only** (single-connection). Never the backend.
- **Cloud-hosted PocketBase — still the zero-rewrite baseline** to beat; its gap
  vs the above is weaker native RBAC/encryption. If those are must-haves, Appwrite
  is the upgrade; if not, PocketBase wins on effort.

### Why the `trait SyncBackend` seam matters MORE given this
The seam lets Appwrite / Supabase / cloud-PocketBase be SWAPPABLE IMPLS evaluated
with a working spike — not three separate 11k-line rewrites decided on paper.
Build the seam first; pick the winner behind it.

## Honest estimate
Multi-week project, dominated NOT by SQL but by rebuilding auth + file storage +
access rules that PocketBase gave for free. The DB swap (PGlite↔Neon↔Lakebase) is
the EASY, late part. Do step 1 (the A/B decision + naming the PG requirement) and
step 2 (the `SyncBackend` seam) before writing any Postgres code — both are
useful regardless and prevent committing to an 11k-line rewrite on a hunch.

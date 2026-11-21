# Spike: replacing rusqlite with Turso for Chatham's local store

Status: SCOPING (no code changes yet). Branch `spike/turso-local-store`.
Author: scoping pass, 2026-11.

## Question
Can we swap Chatham's embedded local database from `rusqlite` (bundled SQLite)
to **Turso** (the Rust SQLite rewrite, crate `turso`), keeping the on-disk file
SQLite-compatible but gaining Turso's engine (MVCC `BEGIN CONCURRENT`,
auto-updating materialized views, async I/O)?

Short answer: **possible but it is a threading-model rewrite, not a dependency
swap.** The version pin everyone worries about is a red herring. The real cost
is that `turso` is **async-only** and Chatham's DB layer is **sync-under-a-mutex**
by explicit design.

## Two separate database roles (don't conflate)
1. **Local embedded store** — `rusqlite = "0.32"` bundled, shared as
   `Arc<Mutex<Connection>>`. THIS is what a Turso swap targets (Turso's SQLite
   frontend is file-compatible).
2. **Sync/collaboration backend** — the "Self-hosted server" field, a
   PocketBase-shaped BaaS the Rust client speaks over HTTP. Turso (incl. the new
   Postgres frontend) does NOT fit this role: it's a bare SQL engine, not a
   collections/auth/file-storage/hooks BaaS. Using Turso-PG here = rewriting the
   entire sync client. OUT OF SCOPE for this spike. See
   `references/self-hosted-sync-backend.md`.

## What the seam actually looks like (measured)
- App crate (`src-tauri/src`): **28 files** touch `rusqlite`, **138**
  `rusqlite::` references. Hotspots: `db.rs` (337 symbol hits — schema,
  migrations, query core), `commands/chat.rs` (23), `mcp/tools.rs` (14),
  `chat/tools.rs` (13), `commands/inbox.rs` (12), `voiceprints.rs` (11),
  `captures.rs` (10).
- Shared type: `type Db = Arc<Mutex<Connection>>` (`src/chat/mod.rs:93`), and
  dozens of free functions take `conn: &rusqlite::Connection`.
- `cloud-sync` crate: in-tree workspace sibling (`path = "crates/cloud-sync"`),
  editable — NOT a published dependency. ~40 `rusqlite::` sites in its
  `lib.rs`. Its concurrency contract (lib.rs:24): *"the shared
  `rusqlite::Connection` is behind a `parking_lot::Mutex`. We NEVER hold that
  guard across an `.await`."*
- `humla_lib` is Chatham's OWN lib name (`src-tauri/Cargo.toml:16`), not an
  external crate. The rusqlite "0.32 pin to match the public core" is about
  merge-compatibility with the UPSTREAM fork, not a dependency we can't change.

## The real blocker: sync → async inversion
`turso` crate (0.4.3) is async-only. Every op is awaited:
```rust
let db = Builder::new_local("data.db").build().await?;
let conn = db.connect()?;
conn.execute("INSERT ...", params).await?;
let mut rows = conn.query("SELECT ...", ()).await?;
while let Some(row) = rows.next().await? { ... }
```
rusqlite is synchronous. So the swap forces:
1. **138 call sites** gain `.await` (and their fns become `async`), rippling the
   async color up through `commands.rs`, `mcp/`, `chat/`, `voiceprints`,
   `captures`, `inbox`, etc.
2. **`Arc<parking_lot::Mutex<Connection>>` must go** — you cannot hold a sync
   mutex across an `.await`. Options:
   - `turso::Connection` is `Clone` + internally concurrent → likely DROP the
     mutex entirely and clone a handle per task (cleanest).
   - or a single owner task + mpsc command channel.
   - or `tokio::Mutex` (simplest textual change, worst throughput).
   This directly rewrites the cloud-sync concurrency contract — the thing the
   crate was carefully built around.
3. **API differences** to port, not just rename:
   - `params![...]` → Turso param binding (arrays/tuples).
   - `query_row`, `prepare_cached`, `OptionalExtension` (`.optional()`),
     row `.get::<i64>(0)?` typed accessors → Turso `Row::get_value` + manual
     typing. Many ergonomic rusqlite helpers have no 1:1 yet.
   - FTS5 (`note_chunks_fts`), triggers, `ALTER` usage in `db.rs` migrations —
     must be verified against Turso's SQLite-compat matrix (tracking 3.50.4).
     FTS5 support in particular MUST be confirmed before committing — Chatham's
     note search depends on it.

## Risk / unknowns to probe before ANY port
- [ ] FTS5 (`CREATE VIRTUAL TABLE ... USING fts5`) — Chatham search depends on
      it. Confirm Turso supports it, or search breaks.
- [ ] Triggers + `ON CONFLICT ... DO UPDATE` (used heavily in sync upserts).
- [ ] `PRAGMA`s Chatham sets at open (WAL, foreign_keys, busy_timeout).
- [ ] Reading an EXISTING rusqlite-written .db file without migration surprises.
- [ ] Bundle size / build time vs bundled-rusqlite; macOS codesign of any new
      dylib (ties into the self-signing skill — new binary = re-sign).
- [ ] Does dropping the mutex actually hold under the sync worker's snapshot
      semantics, or does it introduce read/write races the mutex was hiding?

## Recommended spike shape (de-risk cheaply, DON'T port everything)
Do NOT start converting 138 sites. Instead, smallest proof first:
1. Add `turso` as an OPTIONAL dep behind a `turso-store` feature. Leave
   rusqlite as default. (No commitment, builds stay green.)
2. Write ONE throwaway integration test / small bin that:
   - opens Chatham's real schema from `db.rs` (copy the `CREATE` statements)
     through Turso,
   - exercises FTS5 note search, a sync-style upsert (`ON CONFLICT DO UPDATE`),
     and reads a .db file written by the current rusqlite build.
   - This answers the FTS5 + compat unknowns in an afternoon, before any
     architecture change.
3. ONLY if that passes: prototype the async seam on the SMALLEST isolated
   module (`voiceprints.rs`, 11 sites, few callers) to feel the ripple, measure
   how far async spreads. Decide go/no-go from that.

## Honest recommendation
High effort, modest payoff AT CURRENT SCALE. Chatham is single-user/small-team;
it is not bottlenecked on write concurrency or materialized views today. The
async inversion touches the most delicate part of the codebase (the sync
engine's threading contract) for features not currently needed. Worth a
time-boxed afternoon on step 2 (the compat probe) to learn the facts; NOT worth
a blind full port. Turso-the-Postgres-frontend is irrelevant to Chatham and
remains experimental regardless.

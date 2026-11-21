# Chatham self-host PocketBase schema

Reverse-engineered from the Chatham desktop sync client. This is the exact set of
PocketBase collections + custom API routes the client reads from and writes to, so a
developer can stand up a matching self-hosted sync server.

Every field below is cited to the `file:line` where the client serializes it (push),
parses it (pull/read), or sends it as a query/filter. Types are what PocketBase must
store so the client's JSON round-trips. Fields the client never reads but that are
load-bearing for PocketBase rules (ownership, timestamps) are called out as
**server/inferred** and explained.

Primary paths referenced:
- `src-tauri/crates/cloud-sync/src/lib.rs` — the sync engine (notes/folders/clients/
  summary_prompts/note_sessions/note_revocations push+pull).
- `src-tauri/src/commands/cloud.rs` — auth, workspaces, membership, billing, file sync.
- `src-tauri/src/commands/cloud_auth.rs` — password auth + two-factor status.
- `src-tauri/src/commands/chat.rs` + `src-tauri/src/chat/cloud.rs` — chat routes.

---

## Cross-cutting sync conventions

The engine syncs six collections with a **client-authoritative, last-write-wins**
model (`cloud-sync/src/lib.rs:9-13`):

- **`client_id`** (text): the local SQLite UUID, used as the stable cross-device key.
  PocketBase's own 15-char `id` can't hold a UUID, so every synced record carries a
  separate `client_id`. The client finds a record by filtering
  `client_id='<uuid>' && workspace='<ws>'` (`cloud.rs:1261`, `lib.rs:1116`). Must be
  indexed; effectively unique per `(client_id, workspace)`.
- **`workspace`** (relation → workspaces): the tenant. All reads/pulls filter on it
  (`lib.rs:1113-1118`); Personal/local-only notes use `""` and never sync.
- **`client_updated_at`** (number, epoch-ms): the LWW clock the client sets on every
  push (`lib.rs:579-606`). NOT PocketBase's `updated` autodate.
- **`deleted`** (bool): soft-delete tombstone. Deletes are PATCHes setting
  `deleted:true` + `client_updated_at` (`lib.rs:784`), never hard DELETEs.
- PocketBase's native **`updated`** autodate is the *pull cursor*: the client pages
  `filter=workspace='..' && (updated > 'ts' || (updated='ts' && id>'id'))`,
  `sort=updated,id`, `perPage=200` (`lib.rs:1111-1130`). So `updated` must be a real
  autodate that advances on every write (including the soft-delete PATCH).
- `client_id` values from the server are rejected unless they match `[A-Za-z0-9_-]+`
  (`lib.rs:1675`), so ids must be UUID-shaped.

Create vs update: the client GETs by `(client_id, workspace)`; if found it PATCHes
that record id, else POSTs a new one (`lib.rs:609-626`). So collection create/update
rules must permit both for a workspace member.

---

## Collection: `users` (PocketBase auth collection)

Standard PocketBase `auth` collection. The client uses password auth and reads a few
profile fields.

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `id` | text (PB id) | yes (auto) | User id; stored in workspace relations (`owner`/`members`/`admins`/`viewers`) and note `owner`. |
| `email` | email | yes | Sent as `identity` to `auth-with-password` (`cloud_auth.rs:624`); shown in roster (`cloud.rs:990`). Signup sets `emailVisibility:true` so co-members can read it (`cloud.rs:779`). |
| `password` | password | yes | Signup posts `password`+`passwordConfirm` (`cloud.rs:776-777`). |
| `name` | text | optional | Display name; read for roster + speaker-rename suggestions (`cloud.rs:991`, `cloud_auth.rs:114`). Set at signup (`cloud.rs:778`). |
| `verified` | bool | yes (auto) | PocketBase's standard email-verified flag; read to drive the "verify your email" banner (`cloud_auth.rs:115`). |
| `emailVisibility` | bool | — | Standard PB field; client sets it `true` at signup (`cloud.rs:779`). |

**Rules / behavior the client depends on:**
- `createRule` must allow public sign-up (POST `/api/collections/users/records`,
  `cloud.rs:772-783`).
- `auth-with-password`, `request-verification`, `request-password-reset` endpoints must
  be enabled (`cloud.rs:819`, `cloud.rs:844`, `cloud_auth.rs:623`).
- list/view rules are expected to be **locked down** (no enumeration): the client never
  lists users directly — it resolves email→id via the `/api/chatham/find-user` hook
  (`cloud.rs:1020-1044`). Roster reads come through `workspaces` `expand=members`
  (`cloud.rs:964`), so `members` expansion must return `id,email,name`.
- Two-factor: when enabled the server may require a TOTP second factor; see the 2FA
  routes below. (No `users` schema field is read for this by the client; it's all via
  hook routes.)

---

## Collection: `workspaces`

The tenant/team. Roles are derived entirely from relation membership
(`cloud.rs:14-15, 466-477`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `id` | text (PB id) | yes (auto) | Workspace id; persisted locally as `cloud_workspace_id`. |
| `name` | text | yes | Created/renamed by client (`cloud.rs:876, 1168`); read everywhere (`cloud.rs:537`). |
| `owner` | relation → users (single) | yes | Role=owner. Set to creator on create (`cloud.rs:877`). Immutable via normal API — transfer only via hook. |
| `members` | relation → users (multi) | yes | Drives read access + roster. Create seeds `[creator]` (`cloud.rs:878`). Add/remove by PATCHing the array (`cloud.rs:1050, 1099`). `listRule` is `members.id ?= @request.auth.id` (`cloud.rs:492-497`). |
| `admins` | relation → users (multi) | optional | Role=admin. Create seeds `[creator]` (`cloud.rs:879`); edited via role changes (`cloud.rs:1149`). |
| `viewers` | relation → users (multi) | optional | Role=viewer (read-only). Edited via role changes (`cloud.rs:473-475, 1149`). |
| `require_2fa` | bool | optional | "Every member must have 2FA to open it." Read in status (`cloud.rs:541`); default false. |

**Rules the client depends on:**
- `listRule`: `members.id ?= @request.auth.id` — unfiltered list returns only the user's
  workspaces (`cloud.rs:492-502`). Client does NOT re-send a members filter (it 400s).
- `updateRule`: owner or admin (rename, member add/remove, role changes; `cloud.rs:1155`).
- `deleteRule`: owner only; deleting cascades notes/folders/prompts via the `workspace`
  relation (`cloud.rs:1172-1184`).
- `expand=members` on a single-record GET must return each member's `id,email,name`
  (`cloud.rs:964-974`).

---

## Collection: `notes`

The core synced entity. Pushed by `push_note` (`lib.rs:582-608`), parsed by
`RemoteNote` (`lib.rs:1570-1589`) + `apply_remote_note` (`lib.rs:1190-1318`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `client_id` | text | yes | Note's local UUID (`lib.rs:583`). Find key (`cloud.rs:1261`). Must be unique per workspace, `[A-Za-z0-9_-]+`. |
| `workspace` | relation → workspaces | yes | Tenant (`lib.rs:584`). |
| `owner` | relation → users (single) OR text user id | yes | Note author. Set to pushing user only when creating; preserved on edits so editing a teammate's note doesn't reassign (`lib.rs:563-567, 585`). Read back into local `owner` (`lib.rs:1312`). Server visibility rule keys on this + `private`. |
| `title` | text | optional | `lib.rs:586`, read `lib.rs:1575`. |
| `body` | text (rich/HTML, long) | optional | Note body HTML (`lib.rs:587`). Can be large. |
| `transcript` | text (long) | optional | Meeting transcript (`lib.rs:588`). |
| `summary` | text (long) | optional | AI summary (`lib.rs:589`). |
| `language` | text | optional | Per-note transcription language (`lib.rs:590`). |
| `summary_preset` | text | optional | Preset id (Meeting/1:1/…) (`lib.rs:591`). |
| `folder_client_id` | text | optional | Soft reference: holds the referenced folder's `client_id` (NOT a PB relation). Empty = no folder (`lib.rs:592`, read `lib.rs:1273-1277, 1581`). |
| `client_client_id` | text | optional | Soft reference to a `clients` record's `client_id` (the note's "client" tag, #49). Empty = untagged (`lib.rs:593-597`, read `lib.rs:1281-1285, 1582`). |
| `expected_speakers` | number (int) | optional | Diarization hint; `0` means unset (pushed as `0` when None) (`lib.rs:598`, read `lib.rs:1286, 1583`). |
| `private` | bool | yes | Visibility inside workspace (#191). Server rule keys on this + `owner` (`lib.rs:599-601`, read `lib.rs:1585`). |
| `shared_with` | json (array of user ids) | optional | Who a private note is also readable by (#232). Sent as a JSON array; "only the author's value survives on the server" (`lib.rs:602-604`, read `lib.rs:1586`). |
| `created_at` | number (epoch-ms) | optional | Client-stamped creation time (`lib.rs:605`, read `lib.rs:1587`). Distinct from PB `created`. |
| `client_updated_at` | number (epoch-ms) | yes | LWW clock (`lib.rs:606`). On delete, bumped to `now_ms()` (`lib.rs:784`). |
| `deleted` | bool | yes | Tombstone (`lib.rs:607`); soft-delete PATCH sets it true (`lib.rs:784`). |
| `audio` | file (single) | optional | Legacy single-file per-note audio (`playback.wav`). Uploaded via multipart PATCH field name `audio` (`cloud.rs:1349-1353`); read as filename via `fields=id,audio` (`cloud.rs:1266, 1273`); downloaded through the file-token flow (`cloud.rs:1404-1408`). Protected file field. |
| `updated` | autodate | server | **server/inferred**: PocketBase autodate, used as the pull cursor (`lib.rs:1116, 1144`). Must advance on every write. |
| `created` | autodate | server | **server/inferred**: standard PB autodate (not read by sync; harmless). |

Rules: a workspace member must be able to create/update/list notes in their workspace;
server enforces the `private`/`owner`/`shared_with` visibility policy. Cascade-delete
on `workspace` deletion.

---

## Collection: `folders`

Pushed by `push_folder` (`lib.rs:635-641`); `RemoteFolder` (`lib.rs:1600-1607`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `client_id` | text | yes | Folder local UUID (`lib.rs:636`). Find key / referenced by `notes.folder_client_id`. |
| `workspace` | relation → workspaces | yes | Tenant (`lib.rs:637`). |
| `name` | text | yes | Folder name (`lib.rs:638`, read `lib.rs:1605`). |
| `client_updated_at` | number (epoch-ms) | yes | LWW clock (`lib.rs:639`). |
| `deleted` | bool | yes | Tombstone (`lib.rs:640`; delete PATCH `lib.rs:784`). On pull, a tombstone hard-deletes the local folder (`lib.rs:1332-1337`). |
| `updated` | autodate | server | **server/inferred**: pull cursor (`cursor_key=folders_cursor`, `lib.rs:1048`). |

No separate client `created_at` is synced; the local store seeds it from
`client_updated_at` (`lib.rs:1339-1346`).

---

## Collection: `clients`

A "client"/customer tag a note can reference (#49). Pushed by `push_client`
(`lib.rs:650-656`); `RemoteClient` (`lib.rs:1609-1616`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `client_id` | text | yes | Client local UUID (`lib.rs:651`). Referenced by `notes.client_client_id`. |
| `workspace` | relation → workspaces | yes | Tenant (`lib.rs:652`). |
| `name` | text | yes | Client/customer name (`lib.rs:653`, read `lib.rs:1614`). |
| `client_updated_at` | number (epoch-ms) | yes | LWW clock (`lib.rs:654`). |
| `deleted` | bool | yes | Tombstone (`lib.rs:655`); pull tombstone hard-deletes locally (`lib.rs:1357-1366`). |
| `updated` | autodate | server | **server/inferred**: pull cursor `clients_cursor` (`lib.rs:1051`). |

> Naming note: this collection is called `clients` and its records are "client" tags.
> It is **distinct** from the `client_id` field on every collection (which is a UUID key).

---

## Collection: `summary_prompts`

Custom summary prompts (#…). Pushed by `push_prompt` (`lib.rs:665-671`);
`RemotePrompt` (`lib.rs:1618-1626`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `client_id` | text | yes | Prompt local UUID (`lib.rs:666`). |
| `workspace` | relation → workspaces | yes | Tenant (`lib.rs:667`). |
| `name` | text | yes | Prompt name (`lib.rs:668`, read `lib.rs:1623`). |
| `content` | text (long) | yes | Prompt body (`lib.rs:669`, read `lib.rs:1624`). |
| `client_updated_at` | number (epoch-ms) | yes | LWW clock (`lib.rs:670`). |
| `deleted` | bool | yes | Tombstone (`lib.rs:671`); pull tombstone hard-deletes locally (`lib.rs:1387-1391`). |
| `updated` | autodate | server | **server/inferred**: pull cursor `prompts_cursor` (`lib.rs:1054`). |

PocketBase REST collection name in the URL is `summary_prompts`
(`lib.rs:474, 673, 1054`).

---

## Collection: `note_sessions`

Per-recording-session metadata + binary assets (#16). Metadata pushed by
`push_session` (`lib.rs:711-721`); assets uploaded/parsed in `cloud.rs`
(`parse_remote_session` `cloud.rs:1462-1482`, upload `cloud.rs:1658-1680`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `client_id` | text | yes | Session local UUID (`lib.rs:712`). Globally unique → `(client_id, workspace)` finds it (`lib.rs:482-484`). Used verbatim as a filesystem path segment, so server values are rejected unless safe (`cloud.rs:1521-1543`). |
| `note` | relation → notes (single) | yes | Parent note's **PocketBase record id** (resolved via find-by-client_id first), not the note's client_id (`lib.rs:694-713`). Sessions filter by `note='<pb_id>'` (`cloud.rs:1490`). |
| `workspace` | relation → workspaces | yes | Tenant (`lib.rs:714`). |
| `session_index` | number (int) | optional | Take index (`lib.rs:715`, read `cloud.rs:1471`). Sort key (`cloud.rs:1498`). |
| `started_at` | number (epoch-ms) | optional | Session start, converted from RFC3339 manifest (`lib.rs:709, 716`; read `cloud.rs:1472`). |
| `duration_ms` | number (int) | optional | Take duration (`lib.rs:717`, read `cloud.rs:1473`). |
| `streams` | json (array of strings) | optional | e.g. `["mic","sys"]` (`lib.rs:718`, read `cloud.rs:1474-1478`). |
| `client_updated_at` | number (epoch-ms) | yes | LWW clock; derived from `started_at` (`max 1`) since metadata is immutable (`lib.rs:709-719`). |
| `deleted` | bool | yes | Tombstone (`lib.rs:720`; delete PATCH `lib.rs:784`), read `cloud.rs:1479`. |
| `playback` | file (single) | optional | `playback.wav` (mixed) (`cloud.rs:1465-1466`). Multipart PATCH field `playback`. |
| `mic` | file (single) | optional | `mic_full.wav` raw mic stream (`cloud.rs:1465`). |
| `sys` | file (single) | optional | `sys_full.wav` raw system-audio stream (`cloud.rs:1465`). |
| `timeline` | file (single) | optional | `timeline.jsonl` word-timings (`cloud.rs:1465`; always re-synced). |
| `chunks` | file (single OR archive) | optional | VAD chunk assets (`cloud.rs:1465`). |
| `updated` | autodate | server | **server/inferred**: note_sessions are not on the generic pull list in `pull()` — synced via the app's cloud commands — but still ordered by `session_index` on read. |

Reads request `fields=id,client_id,session_index,started_at,duration_ms,streams,deleted,
playback,mic,sys,timeline,chunks` (`cloud.rs:1500-1502`). All five file fields are
single-file fields whose stored (suffixed) filename the client reads back
(`cloud.rs:1458-1466`) and fetches via the file-token flow.

---

## Collection: `chat_messages`

Server-authoritative workspace chat history (#50), read-through only — the client never
writes here directly (writes happen inside the `/api/chat` turn on the server). Parsed by
`map_remote_message` (`chat.rs:2404-2415`); fetched in `fetch_chat_messages`
(`cloud.rs:437-455`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `id` | text (PB id) | yes (auto) | Message id, surfaced to UI (`chat.rs:2405`). |
| `conversation` | relation → (conversations) | yes | Scopes the fetch: `filter=conversation="<id>"` (`cloud.rs:442-447`). The `<id>` is the server conversation record id (the local handle's `remote_id`). |
| `seq` | number (int) | yes | Ordering key; read `sort=seq` (`cloud.rs:447`, `chat.rs:2407`). |
| `role` | text | yes | `"user"` / `"assistant"` etc. (`chat.rs:2406`). |
| `parts` | json | yes | opencode-v2 parts array, frontend-shaped camelCase, passed through verbatim (`chat.rs:2328-2332, 2408`). |
| `created` | autodate | yes (auto) | RFC3339; converted to epoch-ms for the UI (`chat.rs:2409-2414`). |
| `updated` | autodate | server | **server/inferred**: standard PB autodate. |

**listRule** must be member-readable scoped by the caller's token (`cloud.rs:433-436`).
`perPage=200` (`cloud.rs:447`).

> The server also maintains a **conversations** collection (referenced by
> `chat_messages.conversation` and by the `/api/chatham/chat/conversations` hook). The
> desktop client never touches it via the records API — only via the hook routes — so
> its exact schema isn't fully observable from the client. From the hook payloads
> (`chat.rs:2430-2438`) a conversation carries at least: `id`, `title` (text),
> `breadth` (text: `note|folder|all`), `message_count` (int), `created_by` (relation →
> users), `note` (relation/text, adopted one-way), `workspace` (relation), plus PB
> `created`/`updated`. Treat this as **inferred**, not confirmed by client serialization.

---

## Collection: `subscriptions`

Per-workspace Stripe billing state (chatham-cloud only; self-host has no rows). Read-only
from the client (`cloud.rs:507-525`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `id` | text (PB id) | yes (auto) | — |
| `workspace` | relation → workspaces | yes | Keys the status map (`cloud.rs:518`). |
| `status` | text | yes | `active`/`trialing`/`past_due`/`canceled`/`none` (`cloud.rs:519`). |
| `seats` | number (int or float) | optional | Billed seat count; `<1`/non-numeric → treated as unset (`cloud.rs:483-488, 521`). |
| `updated` | autodate | server | **server/inferred**. |

**listRule** must scope to the caller's workspace memberships so an unfiltered list
(`perPage=200`, `cloud.rs:514`) returns only the user's rows. **On self-host this
collection may be empty/absent** — the client treats any error as "no subscription"
(`cloud.rs:509-511`).

---

## Collection: `note_revocations` (additional, required for full sync)

Not in the original list but **the client pushes and pulls it** (#191/#232). Write:
`push_revocation` POSTs a fresh row each time (`lib.rs:815-822`). Read: pulled last each
cycle and parsed into `RemoteRevocation` (`lib.rs:1062-1065, 1591-1598`).

| Field | Type | Required | Notes (client usage) |
|---|---|---|---|
| `note_client_id` | text | yes | The withdrawn note's `client_id` (`lib.rs:819`, read `lib.rs:1594`). Rejected if unsafe id (`lib.rs:832`). |
| `workspace` | relation → workspaces | yes | Tenant; pulls filter on it (`lib.rs:819, 1113`). |
| `at` | number (epoch-ms) | optional | Server-stamped time; `0`/absent disables the freshness guard (`lib.rs:1595-1597`). **server-stamped** (client doesn't send it). |
| `updated` | autodate | server | **server/inferred**: pull cursor `revocations_cursor` (`lib.rs:1062`). |

It's an append-only "fact about a moment": no find-then-patch; a second withdrawal must
advance `updated` (`lib.rs:790-795`). Server must refuse a revocation for a note the
caller doesn't own.

---

# Custom (non-collection) API routes

These are PocketBase **hook routes** (and a separate chat service for `/api/chat*`). All
take `Authorization: Bearer <pb token>` unless noted. Error shape across the board is
`{ "reason": <code>, "error": <message> }` with meaningful HTTP status (`cloud.rs:415-427`,
`chat.rs:2246-2252`). A **404 means "route absent on this (older/self-hosted) server"**
and the client degrades gracefully.

## `/api/chatham/*` — PocketBase hooks

### `POST /api/chatham/find-user`
Resolve an email to a user id without exposing the users collection.
- Req: `{ "email": "<email>" }` (`cloud.rs:1027-1028`).
- Resp 200: `{ "id": "<pb user id>", … }` (`cloud.rs:1040-1044`).
- 404 when no account exists for that email (`cloud.rs:1033`).

### `POST /api/chatham/invite`
Invite by email; adds immediately if the account exists, else records a pending invite.
Owner/admin only (server-enforced).
- Req: `{ "workspace_id", "email", "role" }` (`cloud.rs:1070-1074`).
- Resp 200: `{ "status": "added" | "invited" }` (`cloud.rs:1079`).

### `POST /api/chatham/leave-workspace`
Member removes self (workspaces updateRule is owner/admin-only so a plain PATCH can't).
Server rejects the owner.
- Req: `{ "workspace_id" }` (`cloud.rs:1210`).
- Resp: 200 (message surfaced on error, e.g. owner-can't-leave) (`cloud.rs:1214`).

### `POST /api/chatham/transfer-workspace`
Transfer ownership; owner-only, new owner must already be a member, promotes them to
admin and keeps the old owner as admin.
- Req: `{ "workspace_id", "new_owner_id" }` (`cloud.rs:1243`).
- Resp: 200 (`cloud.rs:1247`).

### `GET /api/chatham/billing/config`  (no auth required)
Public billing/feature config.
- Resp 200: `{ "enabled": bool, "seat_price_cents"?: number, "seat_currency"?: string,
  "chat_addon"?: { "available": bool, "price_id"?: string, "price_cents"?: number,
  "currency"?: string } }` (`cloud.rs:587-608, 561-581`).
- Self-host returns `{ "enabled": false }` (or 404/unreachable → treated the same).

### `POST /api/chatham/billing/checkout`
Start/resume Stripe Checkout for a workspace.
- Req: `{ "workspace_id", "source"? }` where `source` matches `^[a-z0-9_]{1,40}$`
  (`cloud.rs:710-716`).
- Resp 200: `{ "url": "<hosted checkout url>" }` (`cloud.rs:718-721`).

### `POST /api/chatham/billing/portal`
Open the Stripe customer portal.
- Req: `{ "workspace_id" }` (`cloud.rs:733`).
- Resp 200: `{ "url": "<hosted portal url>" }` (`cloud.rs:736-739`).

### `GET /api/chatham/chat/conversations?workspace_id=<ws>&note_id=<note>`
List workspace conversations (server-authoritative).
- Resp 200: `{ "conversations": [ { "id", "title", "breadth": "note|folder|all",
  "created", "updated", "message_count", "created_by" }, … ] }`, ordered `-updated`
  (`chat.rs:2428-2439, 2678`).
- 403 `{ reason:"not_a_member", error }` for a non-member.

### `POST /api/chatham/chat/conversations`
Create a conversation (dumb/unconditional creator).
- Req: `{ "workspace_id", "note_id", "breadth": "note|folder|all" }` (`chat.rs:2444`).
- Resp 200: a **flat** conversation object, same shape as a list item (no wrapper)
  (`chat.rs:2445-2446`).

### `PATCH /api/chatham/chat/conversations/{remote_id}`
Rename a conversation (#109).
- Req: `{ "workspace_id", "title" }` (`chat.rs:1313`).

### `GET /api/chatham/chat/key/meta?workspace_id=<ws>`
Member-readable BYOK key metadata (never the key).
- Resp 200: `{ "configured": bool, "last4"?: string, "set_by"?: string,
  "set_at"?: string|number, "key_health"?: string }` (`chat/cloud.rs:402-409`).
- 404 → "not activated".

### `POST /api/chatham/chat/key`
Owner-only set/rotate; server test-validates against OpenAI.
- Req: `{ "workspace_id", "api_key" }` (`chat.rs:1724`).
- Resp 200: same `ChatKeyMeta` shape as above.
- Error reasons include `byok_key_invalid`, `byok_validation_unreachable`,
  `byok_validation_failed`, `key_encryption_unconfigured`, `forbidden`, `not_a_member`,
  `unauthorized` (`chat/cloud.rs:418-429`).

### `DELETE /api/chatham/chat/key?workspace_id=<ws>`
Owner-only remove. Resp 200: `ChatKeyMeta` (unconfigured) (`chat.rs:1767-1774`).

### 2FA status route — name ambiguity (verify on your server)
Two call sites reference a 2FA status route under different prefixes:
- `cloud_auth.rs` error handling expects `/api/chatham/2fa/*` (`cloud_auth.rs:231`).
- the sync crate GETs **`/api/humla/2fa/status`** (`lib.rs:1435`).

`GET <prefix>/2fa/status` returns
`{ "available": bool, "enabled": bool, "recovery_codes_left": number,
"required_by": [ { "id", "name" }, … ] }` (`cloud_auth.rs:278-297`, consumed in
`lib.rs:1445-1450`). Given the rebrand from Humla→Chatham, implement it under
**`/api/chatham/2fa/status`** and keep `/api/humla/2fa/status` as an alias if you want the
sync worker's 2FA gate to work. A 404 is read as "server doesn't support 2FA".

## `/api/chat/*` — the chat service (separate from PocketBase in prod)

### `POST /api/chat`  (SSE streaming)
Run a workspace chat turn.
- Req body (`chat/cloud.rs:171-182` + `build_cloud_request`):
  ```json
  {
    "workspace_id": "<ws>",
    "message": "<user text>",
    "scope": {
      "breadth": "note" | "folder" | "all",
      "note_id": "<note client_id, optional>",
      "folder_id": "<folder id, only when breadth=folder>",
      "owner": "<user id, optional pin>",
      "owner_name": "<display, optional>",
      "client": "<clients.client_id, optional pin>",
      "client_name": "<display, optional>",
      "speaker": "<label, optional pin>",
      "note_title": "<display, only breadth=note>",
      "folder_name": "<display, only breadth=folder>"
    },
    "conversation_id": "<remote id, omitted on first turn>",
    "title": "<optional>"
  }
  ```
- Resp: `text/event-stream`, hono `streamSSE` framing `event: <name>\ndata: <json>`
  (`chat/cloud.rs:189-199`). Event payloads carry camelCase fields incl.
  `conversationId` (first frame establishes the server conversation id),
  `messageId`, `blockId`, `delta` (`chat.rs:2268-2316`). Non-SSE 2xx is treated as a
  whole-turn answer (`chat.rs:2255-2282`).
- Preflight error (non-2xx): JSON `{ "reason", "error" }` (`chat.rs:2246-2252`).

### `GET /api/chat/usage?workspace_id=<ws>`
Turn-allowance meter (best-effort).
- Resp 200: `{ "unmetered": bool, "used_turns": number, "cap_turns": number,
  "period_end"?: number }` → UI `{ used, cap, periodEnd }`. `unmetered:true` or any
  error → meter hidden (`chat/cloud.rs:322-330`).

### `GET /api/chat/index-state?workspace_id=<ws>`
Retrieval-index state (best-effort).
- Resp 200: `{ "index_state": "ready" | "empty" | "quarantined" }`
  (`chat/cloud.rs:354-360`). Unknown/malformed → ignored.

## `/api/files/*` — protected file access (PocketBase)

### `POST /api/files/token`
Mint a short-lived file token for protected file fields.
- Req: `{}` (`cloud.rs:1404`).
- Resp 200: `{ "token": "<file token>" }` (`cloud.rs:1405`).

### `GET /api/files/notes/{pb_id}/{filename}?token=<file token>`
Download a note's protected `audio` file (`cloud.rs:1407-1408`).

### `GET /api/files/note_sessions/{pb_id}/{filename}?token=<file token>`
Download a session's protected asset files (`playback`/`mic`/`sys`/`timeline`/`chunks`).
Same token flow; path uses the `note_sessions` collection + stored suffixed filename
(`cloud.rs:1458-1466`, download path mirrors the notes flow).

---

## Also used (standard PocketBase auth endpoints)

- `POST /api/collections/users/auth-with-password` — `{ "identity": <email>,
  "password" }`; 200 → `{ token, record{ id,email,name,verified } }`; 401 with
  `{ reason:"two_factor_required", mfaId }` triggers the TOTP flow (`cloud_auth.rs:623-625,
  125-133`).
- `POST /api/collections/users/records` — public sign-up (`cloud.rs:772-783`).
- `POST /api/collections/users/request-verification` — `{ "email" }` (`cloud.rs:819`).
- `POST /api/collections/users/request-password-reset` — `{ "email" }` (`cloud.rs:844`).

> **MFA note:** on `two_factor_required`, the client re-posts auth with `mfaId` plus a
> TOTP/recovery second factor (`cloud_auth.rs:727-728`). Implement PocketBase OTP/MFA
> accordingly if `require_2fa` workspaces are used; otherwise leave 2FA routes returning
> 404 and the client disables the feature.

## Collection summary

| Collection | Synced by engine | Direction | Key fields |
|---|---|---|---|
| `users` | no (auth) | auth + roster read | email, name, verified |
| `workspaces` | no | read + PATCH relations | owner, members, admins, viewers, require_2fa |
| `notes` | **yes** | push + pull | client_id, workspace, owner, private, shared_with, deleted, client_updated_at, audio(file) |
| `folders` | **yes** | push + pull | client_id, workspace, name |
| `clients` | **yes** | push + pull | client_id, workspace, name |
| `summary_prompts` | **yes** | push + pull | client_id, workspace, name, content |
| `note_sessions` | metadata yes / assets via commands | push + pull | client_id, note(rel), session_index, started_at, duration_ms, streams, 5 file fields |
| `note_revocations` | **yes** | push + pull | note_client_id, workspace, at |
| `chat_messages` | no | read-through | conversation, seq, role, parts, created |
| `subscriptions` | no | read-only | workspace, status, seats |
| `conversations` (inferred) | no | via hooks only | id, title, breadth, message_count, created_by, note, workspace |

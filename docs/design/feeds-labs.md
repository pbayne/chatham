# Feeds (Labs): RSS reader + podcast download → transcribe

Status: DESIGN. Target 0.79.0. Behind a Labs toggle, off by default.

## Decisions (from Patrick, 2026-11-25)
- Unlock: Settings → General → **Labs** section, toggle "Feeds" (setting
  `labs_feeds`, default off). On = a **Feeds** row in the sidebar nav under
  People. Off = route still resolves (deep link) but nav hides it.
- A transcribed episode becomes a **regular note** in an auto-created
  **Podcasts** folder, so search, chat, summaries, MCP and People all work on
  it with no special-casing.

## What already exists (don't rebuild)
- `import_audio(path, language, expected_speakers)` — takes a local file,
  claims the capture slot, replays it through the `audio-capture --import`
  sidecar (AVAudioFile → reads mp3/m4a/aac/wav), transcribes live, diarizes,
  titles, writes playback. A podcast is just "download to a temp file, then
  import_audio". Title is seeded from the filename; we overwrite it with the
  episode title after.
- `reqwest` (rustls, stream) is already a dependency — use it for feed fetch +
  enclosure download (streamed to disk, progress events).
- Folders: `db::list_folders` / `db::create_folder` — find-or-create
  "Podcasts" by name in the Personal workspace.
- Settings: generic `settings` table + `DEFAULTS` map in
  `src/pages/settings/types.ts`; add `labs_feeds` there and to `SettingsKey`.
- Pattern for a feature-local table with no migration coordination:
  `commands/inbox.rs::ensure_table` (CREATE IF NOT EXISTS at top of each cmd).

## Backend (src-tauri/src/feeds.rs + commands/feeds.rs)
Tables (local-only, never synced — like inbox_triage):
- `feeds(id, url UNIQUE, title, site_url, kind 'rss'|'podcast', image_url,
  last_fetched_at, etag, last_modified, error, created_at)`
- `feed_items(id, feed_id, guid, title, link, author, published_at,
  summary_html, content_html, enclosure_url, enclosure_type, enclosure_bytes,
  duration_s, read INTEGER, starred INTEGER, note_id NULL, UNIQUE(feed_id, guid))`

Parsing: add `feed-rs` crate (RSS 0.9–2.0, Atom, JSON Feed, iTunes podcast
ext). Kind = podcast when any item has an audio/* enclosure.

Commands:
- `feeds_list`, `feeds_add(url)` (fetch + parse once; also accepts a site URL
  and autodiscovers `<link rel=alternate type=application/rss+xml>`),
  `feeds_remove(id)`, `feeds_refresh(id?)` (conditional GET with ETag /
  Last-Modified; upsert items by guid), `feeds_import_opml(path)`,
  `feeds_export_opml(path)`.
- `feed_items_list(feed_id?, unread_only, limit, before)`,
  `feed_item_set_read(id, read)`, `feed_items_mark_all_read(feed_id?)`.
- `feed_item_transcribe(id, language, expected_speakers)`: stream the
  enclosure to `$TMPDIR/chatham-podcast-<id>.<ext>` (emit `feed_download_progress`
  {id, done, total}), then call the same body as `import_audio`, then set title
  = episode title, folder = Podcasts, created_at = published_at, body = show
  notes (sanitized HTML), and record `feed_items.note_id`. Temp file deleted
  after the import's playback is written (import copies what it keeps).
  Refuses while a recording/import holds the capture slot (same error text).
- Background refresh: every 60 min while Labs Feeds is on (tokio interval,
  reads the setting each tick). No notifications in v1.

Safety: article HTML is rendered sanitized (strip script/style/iframe/on*,
rewrite links to open in the system browser). No remote images by default
(privacy, matches "local-first"); a per-article "Load images" button.

## Frontend
- `src/pages/Feeds.tsx` at `/feeds` and `/feeds/:feedId`: three-pane like a
  mail client inside the page frame — feed list (with unread counts, Add feed,
  Import OPML), item list (unread dot, date, title, podcast duration), reader
  (title, source, date, sanitized content; for podcasts: show notes +
  **Transcribe episode** button → progress → "Open note" link once done).
- Keyboard: j/k next/prev item, m toggle read, o open original.
- Sidebar: Feeds NavItem gated on `labs_feeds`; Settings → General → Labs.
- Mock scenarios `feeds`, `feeds-podcast`, `feeds-empty` in mockBoot.

## Tests
- Rust: feed parsing fixtures (RSS 2.0, Atom, podcast w/ itunes), guid upsert
  idempotency, OPML round-trip, conditional-GET 304 path, podcast→note wiring
  (folder find-or-create, title/created_at overwrite) with the import mocked.
- TS: Labs toggle hides/shows nav; Feeds page renders lists/reader; transcribe
  button states; sanitizer strips script/on* handlers.

## Phasing (each its own commit, gate green)
1. Labs toggle + hidden nav + empty Feeds page.
2. feeds/feed_items tables, add/list/remove/refresh, feed-rs parsing + tests.
3. Reader UI (lists, unread, sanitized article view, OPML import/export).
4. Podcast download → transcribe → note in Podcasts folder.
5. Background refresh + polish + mock scenarios + release notes 0.79.0.

## Out of scope v1
Push notifications, full-text extraction for truncated feeds, playback of
episodes without transcribing, sync of feeds across devices.

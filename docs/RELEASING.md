# Releasing Chatham to the team

How a new version reaches every teammate's Mac, and what to touch each time.
Written against the real config (2026-11); verify the ⚠️ items before the first
team release.

## How updates reach the team (the mechanism, already built)

1. Each running app calls Tauri's updater `check()` on launch and from the
   "Check for updates" menu (`src/components/Updater.tsx`).
2. `check()` fetches the endpoint in `src-tauri/tauri.conf.json`:
   `https://github.com/pbayne/chatham/releases/latest/download/latest.json`.
3. If `latest.json`'s version > the running version, the app shows an
   "Update available" toast → Install & Restart. It downloads the bundle,
   verifies its signature against the `pubkey` in tauri.conf, swaps it in, and
   relaunches (`ipc.relaunchApp()`).

So a release = publishing a GitHub Release on `pbayne/chatham` whose assets
include a signed bundle and a `latest.json` pointing at it. Nothing teammates do
except click Install.

## Signing — two DIFFERENT signatures, don't confuse them

- **Updater signature (minisign).** ✅ READY. The private key is at
  `~/.tauri/chatham-updater.key` and its public half MATCHES the `pubkey` in
  tauri.conf (verified). This is what makes `downloadAndInstall` accept the
  bundle. It is YOUR OWN key (generated 2026-11 for the fork, key id
  `638B6EE38B065587`), distinct from upstream Humla's key — do NOT regenerate it,
  or existing installs can never auto-update again (they trust the current
  pubkey). Keep it safe and backed up.
- **Apple code signing (Gatekeeper).** ⚠️ DECISION NEEDED. tauri.conf's
  `macOS.signingIdentity` is the **self-signed** cert hash `70304D84…` (see the
  `chatham-self-signed-codesigning` skill). That works on YOUR Mac because the
  cert is in your login Keychain. On a teammate's Mac it is NOT trusted → every
  install/update throws a Gatekeeper "unidentified developer / cannot be opened"
  wall. This is the one open item (left to you). Options:
  - **Developer ID (recommended for a team):** a $99/yr Apple Developer account,
    sign + notarize + staple. Zero friction on every Mac. Changes
    `signingIdentity` to the Developer ID Application cert and adds a notarize
    step to the build.
  - **Stay self-signed:** each teammate must, once per install, right-click →
    Open (or `xattr -dr com.apple.quarantine /Applications/Chatham.app`). Tolerable
    for 2–3 trusted people, ugly beyond that, and re-prompts can recur per update.

## Per-release checklist

1. **Bump the version** in all three (keep them identical):
   - `src-tauri/tauri.conf.json` → `"version"`
   - `package.json` → `"version"`
   - `src-tauri/Cargo.toml` → `version`
   Current: **0.75.0**. (A `releases.ts` entry for 0.76.0 is already staged —
   bump the version strings to 0.76.0 to match it, or renumber both together.)
2. **Add the "what's new" entry** at the TOP of `src/content/releases.ts`
   (newest first). Shape: `version, date, title, summary, visual?, paragraphs[],
   highlights[], issues[]`. `visual` must be one of the union values in the
   `Release` type (reuse an existing one unless you also add its icon). House
   style is plain, user-facing prose — see existing entries. This is the thread
   that renders on Home; it does NOT auto-generate.
3. **Build with updater artifacts** (already on via `createUpdaterArtifacts:true`).
   The build must sign with the updater key:
   ```sh
   export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/chatham-updater.key)"
   export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""   # set if the key has one
   ./node_modules/.bin/tauri build
   ```
   ⚠️ Expect the tauri build's own Apple-codesign step to fail with
   `errSecInternalComponent` under the self-signed cert — the bundle is still
   produced; re-sign it in place per the signing skill. (Goes away with a real
   Developer ID.) Note: the `signingIdentity` hash is machine-specific to this
   Mac's Keychain — a fresh clone or CI won't have it.
   Artifacts land in `src-tauri/target/release/bundle/` — the `.app.tar.gz`,
   its `.app.tar.gz.sig`, and (if configured) a generated `latest.json`.
4. **Publish the GitHub Release** on `pbayne/chatham`, tagged with the new
   version, uploading: the `.app.tar.gz`, its `.sig`, and `latest.json`.
   `latest.json` must carry: the version, release notes/pub_date, and a
   `platforms."darwin-aarch64"` (and/or `x86_64`) entry with the asset `url` and
   the `signature` (contents of the `.sig`). Tauri can emit this for you; if
   hand-writing, match the asset URL to the uploaded file.
   ⚠️ The updater endpoint reads `releases/latest/download/latest.json`, so the
   release must be the **latest, non-draft, non-prerelease**, and the repo's
   releases must be reachable by teammates (public repo = simplest; a private
   repo needs the apps to carry auth, which isn't configured).
5. **Verify** on a second machine (or a clean account): open Chatham on the old
   version → the toast should offer the new one → Install & Restart → it comes
   back on the new version with the new entry on Home.

## Known gaps to decide (not blockers for the mechanism)
- Apple signing path (above) — the only thing standing between "works for me" and
  "clean for the team".
- `humla.team` still appears in the 0.74.0 release prose and may appear elsewhere
  in-app; decide the brand/domain story before external-facing use.
- No CI: releases are manual today. A GitHub Actions release workflow (build +
  sign + draft release) is the natural next step if release cadence picks up;
  it needs the updater key (and, with Developer ID, the Apple cert) as secrets.

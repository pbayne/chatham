# Google setup — one-time, to go live with Autopilot, Docs & follow-up email

The calendar auto-record, "Send to Google Docs", and "Draft follow-up email"
features are built and unit-tested, but they can't authenticate until a Google
Cloud **OAuth client ID** exists and is compiled into the build. This is a
one-time, ~5-minute setup done once by whoever builds the app (not each user).

Until this is done, `CLIENT_ID` in `src-tauri/src/google_calendar.rs` is the
placeholder `REPLACE_ME.apps.googleusercontent.com` and every Google action
fails fast with an auth error.

---

## 1. Create a project + enable the three APIs

1. Go to <https://console.cloud.google.com/> and create a project (e.g. `humla`).
2. Enable each of these (APIs & Services → Library → search → Enable):
   - **Google Calendar API** — Autopilot reads meetings
   - **Google Docs API** — "Send to Google Docs"
   - **Gmail API** — "Draft follow-up email"

## 2. Configure the OAuth consent screen

1. APIs & Services → **OAuth consent screen**.
2. User type **External**, then fill the app name / support email.
3. On **Scopes**, you can leave it empty here — the app requests scopes at
   runtime. (The scopes it uses, for reference, are in `google_calendar.rs`:
   `calendar.events.readonly`, `drive.file`, `gmail.compose`. All three are
   **non-sensitive / non-restricted**, so Google does NOT require app
   verification for them.)
4. On **Test users**, add every Google account that will use the app (you, your
   colleague). While the consent screen is in "Testing" mode, only listed test
   users can authorize — up to 100. This is the step people forget: an account
   not on the list gets an "app is being tested / access denied" screen.
   - (Optional) Publishing the app removes the test-user list. Because all three
     scopes are non-sensitive, publishing does NOT trigger a verification
     review — but Testing mode is fine for a handful of users.

## 3. Create the OAuth client ID

1. APIs & Services → **Credentials** → Create credentials → **OAuth client ID**.
2. Application type: **Desktop app**. Name it (e.g. `humla-desktop`).
3. Create. Copy the **Client ID** (looks like
   `1234567890-abcdef.apps.googleusercontent.com`).

> Desktop-app clients use PKCE and accept any `http://127.0.0.1:<port>` loopback
> redirect, which is exactly what `oauth_loopback.rs` binds — so there is **no**
> redirect URI to register manually, and **no** client secret to embed.

## 4. Put the client ID into the build

The code reads the ID from the `GOOGLE_OAUTH_CLIENT_ID` env var at compile time
(`option_env!` in `google_calendar.rs`), falling back to the placeholder.

For a one-off local build:

```bash
cd ~/Development/chatham
GOOGLE_OAUTH_CLIENT_ID="PASTE_YOUR_CLIENT_ID.apps.googleusercontent.com" \
  ./scripts/build-mcp.sh && \
GOOGLE_OAUTH_CLIENT_ID="PASTE_YOUR_CLIENT_ID.apps.googleusercontent.com" \
  pnpm tauri build
```

For `scripts/build-dmg.sh` / `release.sh`, export it once in the shell (or add
it to `.env.notarise`, which those scripts already source) so every build picks
it up:

```bash
echo 'export GOOGLE_OAUTH_CLIENT_ID="PASTE.apps.googleusercontent.com"' >> .env.notarise
```

> The client ID is **not a secret** for an installed app (PKCE is what protects
> the flow), so embedding it in a distributed build is expected and safe.

## 5. First-run flow (what the user sees)

1. Open Humla → **Settings → Autopilot → Connect Google**.
2. The default browser opens Google's consent screen; sign in and approve the
   calendar / Docs / Gmail access. One consent covers all three features.
3. The browser redirects to the local loopback, Humla captures the code, stores
   the refresh token in the Keychain, and the panel flips to "Connected".
4. From then on:
   - **Autopilot** (Off/Ask/Auto) watches the calendar and records meetings.
   - A note's **More menu → Send to Google Docs** creates a formatted Doc and
     opens it.
   - A note's **More menu → Draft follow-up email** drafts the email locally and
     opens it in Gmail Drafts for review + send.

## 6. Verifying it actually works (the live test)

With a real client ID compiled in and your account added as a test user:

1. Connect Google in Settings. It should reach "Connected" without error.
2. Settings → Autopilot should list your real upcoming meetings with a
   will-record / skip verdict (proves the Calendar read + parse end-to-end).
3. Open any note with a summary → More → **Send to Google Docs** → the created
   Doc opens with the title/summary/transcript formatted. (proves Docs API)
4. More → **Draft follow-up email** → Gmail Drafts opens with a drafted email.
   (proves local-LLM generation + Gmail draft creation)

If step 1 fails with an "access denied / app not verified" screen, the account
isn't on the test-user list (step 2.4). If it fails with `invalid_client`, the
client ID wasn't compiled in (step 4). If a Workspace admin blocks third-party
OAuth, connection fails regardless — that's an org policy, not a bug.

---

### Troubleshooting map

| Symptom | Cause | Fix |
|---|---|---|
| "Access blocked: app is being tested" | account not a test user | add it (step 2.4) |
| `invalid_client` / `unauthorized_client` | client ID missing/placeholder | rebuild with `GOOGLE_OAUTH_CLIENT_ID` (step 4) |
| Connects, but Docs/Gmail 403 | that API not enabled | enable it (step 1) |
| Connection fails on a corp account | Workspace blocks 3rd-party OAuth | admin policy — test with a personal account or get an exception |

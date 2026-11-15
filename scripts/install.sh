#!/usr/bin/env bash
# install.sh — one-step installer for Humla on a teammate's Mac.
#
# Does everything needed for a fully local, offline-capable Humla:
#   1. Downloads + installs the latest notarised Humla.app (no auth, no Xcode).
#   2. Pre-stages the on-device Whisper model so transcription works instantly.
#   3. Installs Ollama + pulls the summary model so AI summaries run locally.
#
# The repo is public and the app is notarised, so there's no "unidentified
# developer" dance. Everything runs offline after this — no API keys.
#
# Usage (either works):
#   curl -fsSL https://raw.githubusercontent.com/michaelwilhelmsen/humla/main/scripts/install.sh | bash
#   ./scripts/install.sh
#
# Options (env vars):
#   LAUNCH=1       launch Humla when the install finishes
#   VERSION=x      install a specific version tag's DMG (e.g. 0.75.0)
#   NO_LOCAL=1     app only — skip Whisper model + Ollama (use cloud providers)
#   LLM_MODEL=x    Ollama summary model to pull (default: phi3:latest)
set -euo pipefail

REPO="michaelwilhelmsen/humla"
APP="Humla.app"
APP_DEST="/Applications/$APP"
APP_SUPPORT="$HOME/Library/Application Support/no.humla.app"
MODELS_DIR="$APP_SUPPORT/models"

# On-device Whisper model Humla defaults to (large-v3-turbo, quantized).
WHISPER_FILE="ggml-large-v3-turbo-q5_0.bin"
WHISPER_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$WHISPER_FILE"

# Local summary LLM, served by Ollama.
LLM_MODEL="${LLM_MODEL:-phi3:latest}"

note() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m[ok]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarn:\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# 1. Platform guard — Humla ships Apple Silicon (aarch64) only.
[[ "$(uname -s)" == "Darwin" ]] || die "Humla is macOS only (this is $(uname -s))."
ARCH="$(uname -m)"
if [[ "$ARCH" != "arm64" ]]; then
  die "Humla is built for Apple Silicon (arm64); this Mac is $ARCH. No Intel build is published."
fi

# 2. Resolve the download URL for the DMG asset.
if [[ -n "${VERSION:-}" ]]; then
  DMG_URL="https://github.com/$REPO/releases/download/v${VERSION}/Humla_${VERSION}_aarch64.dmg"
  note "Installing pinned version v${VERSION}"
else
  note "Finding the latest Humla release…"
  DMG_URL="$(
    curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
      | grep -oE 'https://[^"]*aarch64\.dmg' | head -n1
  )" || true
  [[ -n "$DMG_URL" ]] || die "Could not find a DMG in the latest release. Check https://github.com/$REPO/releases"
fi
note "Downloading: $DMG_URL"

# 3. Download to a temp dir that always gets cleaned up.
WORK="$(mktemp -d)"
MOUNT=""
cleanup() {
  [[ -n "$MOUNT" && -d "$MOUNT" ]] && hdiutil detach "$MOUNT" -quiet >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

DMG="$WORK/Humla.dmg"
curl -fL --progress-bar -o "$DMG" "$DMG_URL" || die "Download failed."
ok "Downloaded $(du -h "$DMG" | cut -f1)"

# 4. Mount the DMG (private mount point so we always know where it landed).
note "Mounting disk image…"
MOUNT="$WORK/mnt"
mkdir -p "$MOUNT"
hdiutil attach "$DMG" -nobrowse -quiet -mountpoint "$MOUNT" \
  || die "Failed to mount the DMG."
[[ -d "$MOUNT/$APP" ]] || die "$APP not found inside the DMG."

# 5. Replace any existing install, then copy in the fresh app.
if [[ -e "$APP_DEST" ]]; then
  note "Removing previous install at ${APP_DEST}…"
  rm -rf "$APP_DEST" || die "Could not remove old $APP_DEST (try: sudo rm -rf \"$APP_DEST\")."
fi
note "Copying ${APP} to /Applications…"
cp -R "$MOUNT/$APP" "$APP_DEST" || die "Copy failed."
xattr -dr com.apple.quarantine "$APP_DEST" >/dev/null 2>&1 || true

# 6. Verify Gatekeeper accepts it.
note "Verifying signature + notarisation…"
if spctl -a -vvv "$APP_DEST" 2>&1 | grep -q "accepted"; then
  VER="$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' "$APP_DEST/Contents/Info.plist" 2>/dev/null || echo '?')"
  ok "Humla $VER installed and verified — Gatekeeper accepted."
else
  die "Installed, but Gatekeeper did not accept the app. Do not run it; re-download."
fi

# ─── Local model setup (skip with NO_LOCAL=1) ────────────────────────────────
if [[ "${NO_LOCAL:-0}" == "1" ]]; then
  warn "NO_LOCAL=1 — skipping Whisper model + Ollama. Configure a cloud provider in Settings."
else
  # 7. Pre-stage the on-device Whisper model so transcription is ready instantly.
  #    (Humla can download this itself from Settings, but staging it here means
  #     she never waits on a 600 MB download at first use.)
  mkdir -p "$MODELS_DIR"
  if [[ -f "$MODELS_DIR/$WHISPER_FILE" ]]; then
    ok "Whisper model already present ($(du -h "$MODELS_DIR/$WHISPER_FILE" | cut -f1))."
  else
    note "Downloading on-device Whisper model (~600 MB, one-time)…"
    if curl -fL --progress-bar -o "$MODELS_DIR/$WHISPER_FILE.part" "$WHISPER_URL"; then
      mv "$MODELS_DIR/$WHISPER_FILE.part" "$MODELS_DIR/$WHISPER_FILE"
      ok "Whisper model staged → $MODELS_DIR/$WHISPER_FILE"
    else
      rm -f "$MODELS_DIR/$WHISPER_FILE.part"
      warn "Whisper model download failed. Humla will offer to download it in Settings → Transcription."
    fi
  fi

  # 8. Install Ollama (local LLM runtime for summaries) if missing.
  if command -v ollama >/dev/null 2>&1; then
    ok "Ollama already installed ($(ollama --version 2>/dev/null | head -n1))."
  else
    note "Installing Ollama (local LLM runtime for summaries)…"
    if command -v brew >/dev/null 2>&1; then
      brew install --cask ollama >/dev/null 2>&1 && ok "Ollama installed via Homebrew." \
        || warn "brew install ollama failed — install manually from https://ollama.com/download"
    else
      warn "Homebrew not found. Install Ollama from https://ollama.com/download, then re-run this script."
    fi
  fi

  # 9. Start the Ollama server and pull the summary model.
  if command -v ollama >/dev/null 2>&1; then
    if ! curl -s --max-time 2 http://localhost:11434/api/tags >/dev/null 2>&1; then
      note "Starting Ollama server…"
      (ollama serve >/dev/null 2>&1 &) || true
      for _ in $(seq 1 15); do
        curl -s --max-time 1 http://localhost:11434/api/tags >/dev/null 2>&1 && break
        sleep 1
      done
    fi
    if curl -s --max-time 2 http://localhost:11434/api/tags >/dev/null 2>&1; then
      note "Pulling summary model '$LLM_MODEL' (one-time)…"
      ollama pull "$LLM_MODEL" && ok "Summary model '$LLM_MODEL' ready." \
        || warn "Could not pull '$LLM_MODEL'. Run: ollama pull $LLM_MODEL"
    else
      warn "Ollama server didn't come up. Start it (open the Ollama app) then run: ollama pull $LLM_MODEL"
    fi
  fi
fi

echo
ok "Done. Humla is in your Applications folder."
echo "   Launch it from Spotlight (⌘-Space → \"Humla\") or Launchpad."
echo "   On first run macOS asks for Microphone + Screen Recording — allow both."
if [[ "${NO_LOCAL:-0}" != "1" ]]; then
  echo
  echo "   Local setup is done: Settings → Transcription uses on-device Whisper,"
  echo "   and summaries run through Ollama ($LLM_MODEL) at localhost:11434."
  echo "   Everything works offline — no API keys needed."
fi

if [[ "${LAUNCH:-0}" == "1" ]]; then
  note "Launching Humla…"
  open "$APP_DEST"
fi

#!/usr/bin/env bash
# Build the Swift mic-monitor sidecar and place it where Tauri expects.
# Mirror of build-diarize.sh — same hash-based skip, same Developer ID signing
# path. mic-monitor only reads CoreAudio process-object metadata (which app is
# capturing the mic); it does not open an audio stream, so it needs no
# audio-input entitlement, but it does need the hardened runtime + Developer ID
# to pass notarisation.
set -euo pipefail

cd "$(dirname "$0")/.."

ARCH=$(uname -m)
case "$ARCH" in
  arm64)  TRIPLE="aarch64-apple-darwin" ;;
  x86_64) TRIPLE="x86_64-apple-darwin" ;;
  *) echo "unsupported arch $ARCH"; exit 1 ;;
esac

mkdir -p src-tauri/binaries
DEST="src-tauri/binaries/mic-monitor-$TRIPLE"
STAMP="src-tauri/binaries/.mic-monitor-$TRIPLE.stamp"

# Pull the Developer ID from tauri.conf.json so we have one source of truth.
IDENTITY=$(node -e "
  const c = require('./src-tauri/tauri.conf.json');
  process.stdout.write((c.bundle && c.bundle.macOS && c.bundle.macOS.signingIdentity) || '');
")

# Hash includes the signing identity so changing it invalidates the cache.
SRC_HASH=$(
  {
    find mic-monitor/Sources mic-monitor/Package.swift mic-monitor/Info.plist -type f \
      \( -name '*.swift' -o -name 'Package.swift' -o -name 'Info.plist' \) -print0 \
      | sort -z \
      | xargs -0 shasum -a 256
    echo "identity:$IDENTITY"
  } | shasum -a 256 | awk '{print $1}'
)

FORCE="${FORCE_SIDECAR_REBUILD:-0}"
if [[ "$FORCE" != "1" && -f "$DEST" && -f "$STAMP" && "$(cat "$STAMP")" == "$SRC_HASH" ]]; then
  echo "mic-monitor sidecar unchanged, skipping rebuild ($DEST)"
  exit 0
fi

(
  cd mic-monitor
  swift build -c release
)

cp mic-monitor/.build/release/mic-monitor "$DEST"
chmod +x "$DEST"

xattr -cr "$DEST" || true

if [[ -n "$IDENTITY" ]] && security find-identity -v -p codesigning | grep -qF "$IDENTITY"; then
  echo "signing mic-monitor sidecar with: $IDENTITY"
  codesign --force --options runtime \
    --sign "$IDENTITY" \
    --timestamp \
    "$DEST"
else
  echo "warning: Developer ID identity not in Keychain, falling back to ad-hoc signing"
  codesign --force --sign - "$DEST"
fi

echo "$SRC_HASH" > "$STAMP"
echo "built: $DEST"

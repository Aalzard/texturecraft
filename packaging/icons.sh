#!/usr/bin/env bash
# Regenerate every app icon from assets/app-icon/icon.svg (written by assets/app-icon/icon.py).
#
# Needs: resvg (cargo install resvg / brew install resvg) and cargo (for `cargo xtask ico|icns`).
# The outputs are committed, so packaging never needs these tools.
#
#   python3 assets/app-icon/icon.py && packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/icon.svg"
ID=ai.storyteller.texturecraft
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (cargo install resvg)" >&2; exit 1; }

# The artwork is a full-bleed 512-unit tile (rx=112). macOS icons pad it to Apple's 824/1024 body
# grid (transparent margin). Windows and Linux icons crop 22 units off each side (into the
# rounded corners) so the portrait reads at 16-48 px.
grep -q 'viewBox="0 0 512 512"' "$SVG" || { echo "error: expected viewBox=\"0 0 512 512\" in $SVG" >&2; exit 1; }
MAC="$TMP/mac.svg"
sed 's/viewBox="0 0 512 512"/viewBox="-62 -62 636 636"/' "$SVG" >"$MAC"
TIGHT="$TMP/tight.svg"
sed 's/viewBox="0 0 512 512"/viewBox="22 22 468 468"/' "$SVG" >"$TIGHT"

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

# The full tile at 1024 px (README, stores, web).
render "$SVG" 1024 "$DIR/icon-1024.png"

# Linux hicolor theme (16-512 px plus the scalable SVG); also the AppImage and window icons.
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$TIGHT" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$SVG" "$DIR/hicolor/scalable/apps/$ID.svg"

xtask() { (cd "$ROOT" && cargo run -q -p xtask -- "$@"); }

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$TIGHT" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
xtask ico "$DIR/texturecraft.ico" "${ICO_PNGS[@]}"

# macOS .icns (the same PNG slots iconutil writes).
ICNS_PNGS=()
for s in 16 32 64 128 256 512 1024; do
  render "$MAC" "$s" "$TMP/icns-$s.png"
  ICNS_PNGS+=("$TMP/icns-$s.png")
done
xtask icns "$DIR/texturecraft.icns" "${ICNS_PNGS[@]}"
echo "icons written to $DIR"

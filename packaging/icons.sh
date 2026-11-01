#!/usr/bin/env bash
# Regenerate every app icon from assets/app-icon/texturecraft.svg (the canonical artwork).
#
# Needs: resvg (cargo install resvg / brew install resvg) and cargo (for `cargo xtask ico|icns`).
# The outputs are committed, so packaging never needs these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/texturecraft.svg"
ID=ai.storyteller.texturecraft
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (cargo install resvg)" >&2; exit 1; }

# The SVG is a full-bleed 512 tile, used as-is on Windows and Linux. macOS icons follow Apple's grid:
# the tile is 824/1024 of the canvas with a transparent margin, so widen the viewBox around it.
MAC="$TMP/mac.svg"
sed 's/viewBox="0 0 512 512"/viewBox="-62.136 -62.136 636.272 636.272"/' "$SVG" >"$MAC"
grep -q 'viewBox="-62.136' "$MAC" || { echo "error: $SVG has no 512 viewBox" >&2; exit 1; }

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

# The full tile at 1024 px (docs, stores).
render "$SVG" 1024 "$DIR/texturecraft-1024.png"

# Linux hicolor theme (the 256 one is also the runtime window icon and the README icon).
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$DIR/texturecraft-small.svg" "$DIR/hicolor/scalable/apps/$ID.svg"

xtask() { (cd "$ROOT" && cargo run -q -p xtask -- "$@"); }

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$SVG" "$s" "$TMP/ico-$s.png"
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

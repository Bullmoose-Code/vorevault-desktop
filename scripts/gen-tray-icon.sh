#!/usr/bin/env bash
# Generate src-tauri/icons/tray.png from icon.png as a black-on-transparent
# silhouette. Treats the alpha channel as the silhouette mask: any
# non-transparent pixel becomes opaque black; transparent stays transparent.
# Lanczos-downscales to 22x22.
#
# Usage: scripts/gen-tray-icon.sh
# Requires: ImageMagick (convert).

set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
src="$repo_root/src-tauri/icons/icon.png"
dst="$repo_root/src-tauri/icons/tray.png"

if [ ! -f "$src" ]; then
  echo "source icon not found: $src" >&2
  exit 1
fi

if ! command -v convert >/dev/null 2>&1; then
  echo "ImageMagick 'convert' is required" >&2
  exit 1
fi

# 1. Force RGBA output.
# 2. Replace all RGB with black (#000000), preserving alpha.
# 3. Lanczos downscale to 22x22.
# 4. Force RGB type with alpha so the output is true RGBA, not gray+alpha
#    (ImageMagick auto-detects single-color images as grayscale otherwise).
convert "$src" \
  -alpha set \
  -fill black -colorize 100 \
  -filter Lanczos -resize 22x22 \
  -define png:color-type=6 \
  -type TrueColorAlpha \
  "$dst"

echo "wrote $dst"
file "$dst"

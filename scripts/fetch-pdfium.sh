#!/usr/bin/env bash
# Download a prebuilt libpdfium.so (bblanchon/pdfium-binaries) into vendor/.
set -euo pipefail
TAG="${PDFIUM_TAG:-chromium/8100}"
DIR="$(cd "$(dirname "$0")/.." && pwd)/vendor"
mkdir -p "$DIR"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/${TAG}/pdfium-linux-x64.tgz" -o "$tmp/pdfium.tgz"
tar -xzf "$tmp/pdfium.tgz" -C "$tmp"
cp "$tmp/lib/libpdfium.so" "$DIR/"
echo "libpdfium.so ($TAG) -> $DIR"

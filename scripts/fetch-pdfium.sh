#!/usr/bin/env bash
# Download a prebuilt PDFium (bblanchon/pdfium-binaries) into vendor/:
# libpdfium.so on Linux, pdfium.dll on Windows (Git Bash), plus its licence.
set -euo pipefail
TAG="${PDFIUM_TAG:-chromium/8100}"
DIR="$(cd "$(dirname "$0")/.." && pwd)/vendor"
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) PKG=pdfium-win-x64.tgz; LIB=bin/pdfium.dll ;;
  *) PKG=pdfium-linux-x64.tgz; LIB=lib/libpdfium.so ;;
esac
mkdir -p "$DIR"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://github.com/bblanchon/pdfium-binaries/releases/download/${TAG}/${PKG}" -o "$tmp/pdfium.tgz"
tar -xzf "$tmp/pdfium.tgz" -C "$tmp"
cp "$tmp/$LIB" "$DIR/"
cp "$tmp/LICENSE" "$DIR/PDFIUM-LICENSE" 2>/dev/null || true
echo "$(basename "$LIB") ($TAG) -> $DIR"

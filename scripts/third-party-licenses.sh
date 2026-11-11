#!/usr/bin/env bash
# Regenerate THIRD-PARTY-LICENSES.html (bundled by the installer) after changing dependencies.
# Needs cargo-about: cargo install --locked --features cli cargo-about
set -euo pipefail
cd "$(dirname "$0")/.."
cargo about generate --fail about.hbs -o THIRD-PARTY-LICENSES.html

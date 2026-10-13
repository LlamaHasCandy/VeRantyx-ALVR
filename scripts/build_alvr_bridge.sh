#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$REPO_ROOT/ALVRBridge/Cargo.toml"
CARGO_BIN="${CARGO:-cargo}"

if ! command -v "$CARGO_BIN" >/dev/null 2>&1; then
    echo "Rust/Cargo is required to build the ALVR bridge. Install it from https://rustup.rs or with Homebrew." >&2
    exit 1
fi

"$CARGO_BIN" build --release --manifest-path "$MANIFEST"
mkdir -p "$REPO_ROOT/build"
cp "$REPO_ROOT/ALVRBridge/target/release/verantyx-alvr-bridge" "$REPO_ROOT/build/verantyx-alvr-bridge"
echo "Built: $REPO_ROOT/build/verantyx-alvr-bridge"

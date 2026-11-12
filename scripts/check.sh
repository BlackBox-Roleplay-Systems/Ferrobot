#!/usr/bin/env bash
# Formatting, clippy (no warnings) and the tests of the whole workspace. Run before every PR.
set -euo pipefail
cd "$(dirname "$0")/.."
echo "== cargo fmt --check"
cargo fmt --all -- --check
echo "== cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings
echo "== cargo test"
cargo test --workspace
echo "== all good"

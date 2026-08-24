#!/usr/bin/env bash
# Local CI smoke: fmt + clippy + unit tests + example e2e (same spirit as .github/workflows/ci.yml).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

echo "== scripts/check_editor_assets.sh =="
"$ROOT/scripts/check_editor_assets.sh"

echo "== cargo fmt --all -- --check =="
cargo fmt --all -- --check

echo "== cargo clippy --workspace --exclude lumi --lib -- -D warnings =="
cargo clippy --workspace --exclude lumi --lib -- -D warnings

echo "== cargo test --workspace --exclude lumi --lib =="
cargo test --workspace --exclude lumi --lib

# Integration tests that live outside --lib (Core IR goldens, etc.)
echo "== cargo test -p lumi_opt --tests =="
cargo test -p lumi_opt --tests

echo "== cargo test -p lumi --tests (e2e + opt_correctness fingerprints) =="
cargo test -p lumi --tests

echo "OK: check passed"

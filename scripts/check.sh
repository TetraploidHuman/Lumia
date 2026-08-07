#!/usr/bin/env bash
# Local CI smoke: unit tests + example e2e (same spirit as .github/workflows/ci.yml).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

echo "== cargo test --workspace --exclude lumia --lib =="
cargo test --workspace --exclude lumia --lib

echo "== cargo test -p lumia --tests =="
cargo test -p lumia --tests

echo "OK: check passed"

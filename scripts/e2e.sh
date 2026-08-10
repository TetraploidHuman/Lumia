#!/usr/bin/env bash
# Quick end-to-end smoke: build compiler/runtime, then run the cargo e2e suite.
# Full coverage lives in `cargo test -p lumia --test e2e_examples` (not duplicated here).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=env.sh
source "$ROOT/scripts/env.sh"

cd "$ROOT"
cargo build -p lumia -p lumia_rt
cargo test -p lumia --test e2e_examples

echo "OK: e2e smoke passed"

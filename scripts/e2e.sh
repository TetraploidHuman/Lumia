#!/usr/bin/env bash
# Informal smoke only — NOT part of CI / scripts/check.sh.
# Gate: `cargo test -p lumia --tests` (wider than e2e_examples alone).
# Full coverage lives in `cargo test -p lumia --test e2e_examples` plus other
# integration tests under crates/lumia/tests/.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=env.sh
source "$ROOT/scripts/env.sh"

cd "$ROOT"
cargo build -p lumia -p lumia_rt
cargo test -p lumia --test e2e_examples

echo "OK: informal e2e smoke passed (use ./scripts/check.sh for the real gate)"

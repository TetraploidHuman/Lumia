#!/usr/bin/env bash
# Local CI smoke: fmt + clippy + unit tests + example e2e (same spirit as .github/workflows/ci.yml).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

# Match CI: Linux uses llvm-dynamic; Windows/static stays prefer-static.
# Workspace feature path: `lumia_codegen/llvm-dynamic` (not for -p lumia_opt --tests).
LLVM_WS_FEATURES=()
LUMIA_FEATURES=()
case "$(uname -s)" in
  Linux)
    LLVM_WS_FEATURES=(--features lumia_codegen/llvm-dynamic)
    LUMIA_FEATURES=(--features llvm-dynamic)
    ;;
esac

echo "== scripts/check_editor_assets.sh =="
"$ROOT/scripts/check_editor_assets.sh"

echo "== scripts/check_rt_lock_order.sh =="
"$ROOT/scripts/check_rt_lock_order.sh"

echo "== cargo fmt --all -- --check =="
cargo fmt --all -- --check

echo "== cargo clippy --workspace --exclude lumia --lib ${LLVM_WS_FEATURES[*]:-} -- -D warnings =="
cargo clippy --workspace --exclude lumia --lib "${LLVM_WS_FEATURES[@]}" -- -D warnings

echo "== cargo test --workspace --exclude lumia --lib (RUST_TEST_THREADS=1: shared process heap) =="
# Process heap is shared across test cases; parallel lib tests would UAF.
RUST_TEST_THREADS=1 cargo test --workspace --exclude lumia --lib "${LLVM_WS_FEATURES[@]}"

# Integration tests that live outside --lib (Core IR goldens, etc.).
# Do not pass llvm-dynamic here: lumia_opt has no codegen feature (CI same).
echo "== cargo test -p lumia_opt --tests =="
cargo test -p lumia_opt --tests

echo "== cargo test -p lumia --tests (e2e + opt_correctness fingerprints; gate vs informal scripts/e2e.sh) =="
cargo test -p lumia --tests "${LUMIA_FEATURES[@]}"

echo "== slim lumia (no codegen): build + check/lsp smoke =="
# Dedicated target dir so this never overwrites the full LLVM binary (install.sh uses /tmp).
CARGO_TARGET_DIR="$ROOT/target/slim-lsp" cargo build -p lumia --no-default-features
SLIM="$ROOT/target/slim-lsp/debug/lumia"
test -x "$SLIM"
HELP="$("$SLIM" --help)"
echo "$HELP" | grep -q 'check'
echo "$HELP" | grep -q 'lsp'
# `build` / `run` are cfg'd out without the codegen feature.
if echo "$HELP" | grep -qE '(^| )build( |$)'; then
  echo "slim binary unexpectedly exposes build" >&2
  exit 1
fi
"$SLIM" check "$ROOT/examples/hello.lm"
"$SLIM" lsp --help >/dev/null

echo "OK: check passed"

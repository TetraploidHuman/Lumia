#!/usr/bin/env bash
# Compare Release builds with transparent Memo `T_f` on vs off (same LLVM opt level).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

cd "$ROOT"
cargo build -q -p lumia
LUMIA="$ROOT/target/debug/lumia"
SRC=examples/bench_memo.lumia
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_memo"
mkdir -p "$OUT_DIR"
WITH="$OUT_DIR/with_memo"
WITHOUT="$OUT_DIR/without_memo"

echo "== build Release + Memo T_f =="
"$LUMIA" build --release "$SRC" -o "$WITH"
echo "== build Release + --no-memo =="
"$LUMIA" build --release --no-memo "$SRC" -o "$WITHOUT"

# Confirm Slots T_f attached / skipped (IR: memo=Some(Slots …) / memo=None)
echo "== IR memo flag =="
"$LUMIA" build --release "$SRC" -o /dev/null --show-ir 2>/dev/null | grep -E 'fun heavy|memo=' | head -3 || true
"$LUMIA" build --release --no-memo "$SRC" -o /dev/null --show-ir 2>/dev/null | grep -E 'fun heavy|memo=' | head -3 || true

out_with="$("$WITH")"
out_without="$("$WITHOUT")"
if [[ "$out_with" != "$out_without" ]]; then
  echo "checksum mismatch: with=[$out_with] without=[$out_without]" >&2
  exit 1
fi
echo "checksum ok: $out_with"

best_of_three() {
  local bin=$1
  local best=999999
  local i t
  for i in 1 2 3; do
    t="$(TIMEFORMAT='%R'; { time "$bin" >/dev/null; } 2>&1)"
    best="$(awk -v a="$best" -v b="$t" 'BEGIN{ if (b+0 < a+0) print b; else print a }')"
  done
  echo "$best"
}

echo "== timing (best of 3 wall-clock seconds) =="
t_with="$(best_of_three "$WITH")"
t_without="$(best_of_three "$WITHOUT")"
printf 'with Memo T_f:    %ss\n' "$t_with"
printf 'without Memo T_f: %ss\n' "$t_without"
awk -v w="$t_with" -v o="$t_without" 'BEGIN{
  if (w+0 <= 0) { print "speedup: n/a"; exit }
  printf "speedup (without / with): %.2fx\n", o/w
}'

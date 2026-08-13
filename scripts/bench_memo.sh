#!/usr/bin/env bash
# Compare Release builds with transparent Memo `T_f` on vs off (same LLVM opt level).
# Reports wall time and peak RSS (best-of / multi-sample).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumia
LUMIA="$ROOT/target/debug/lumia"
SRC=examples/bench_memo.lm
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_memo"
mkdir -p "$OUT_DIR"
WITH="$OUT_DIR/with_memo"
WITHOUT="$OUT_DIR/without_memo"
RUNS="${RUNS:-5}"

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

measure_n() {
  local bin=$1
  local i samples=""
  for ((i = 0; i < RUNS; i++)); do
    samples+="$(bench_measure "$bin")"$'\n'
  done
  printf '%s' "$samples" | bench_measure_stats
}

echo "== timing + peak RSS (RUNS=$RUNS) =="
s_with="$(measure_n "$WITH")"
s_without="$(measure_n "$WITHOUT")"
bench_print_stats "memo_on" "$s_with"
bench_print_stats "memo_off" "$s_without"
python3 - "$s_with" "$s_without" <<'PY'
import sys
w, o = sys.argv[1].split(), sys.argv[2].split()
wt, ot = float(w[1]), float(o[1])
wr, or_ = float(w[4]), float(o[4])
print(f"speedup  {ot/wt:.2f}x  (memo_off_med_time / memo_on_med_time)")
print(f"rss_ratio {or_/wr:.2f}x  (memo_off_med_rss / memo_on_med_rss)")
PY

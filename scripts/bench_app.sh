#!/usr/bin/env bash
# Production-shaped application microbench (Release).
# Scenarios — see examples/bench/bench_app.lm + bench_str.lm:
#   word_freq, pipe_hof, map_bulk, set_churn, str_pipeline
#
# Env:
#   RUNS=7               # wall-clock samples (default 7)
#   SKIP_STR=1           # skip string-surface binary
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumia_rt --release
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_app"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/bench_app"
BIN_STR="$OUT_DIR/bench_str"
RUNS="${RUNS:-7}"

echo "======== lumia bench_app (Release) ========"
echo "building examples/bench/bench_app.lm → $BIN"
"$LUMIA" build --release examples/bench/bench_app.lm -o "$BIN"

expect_app=(
  869387490
  965428807
  498702828
  1433426
)
labels_app=(
  "word_freq(20k/8)"
  "pipe_hof(2M)"
  "map_bulk(20k)"
  "set_churn(100k)"
)

echo "== checksums (app) =="
mapfile -t LINES < <("$BIN")
if [[ ${#LINES[@]} -ne ${#expect_app[@]} ]]; then
  echo "expected ${#expect_app[@]} checksum lines, got ${#LINES[@]}" >&2
  printf '%s\n' "${LINES[@]}" >&2
  exit 1
fi
for i in "${!expect_app[@]}"; do
  printf '  %-22s %s\n' "${labels_app[$i]}:" "${LINES[$i]}"
  if [[ "${LINES[$i]}" != "${expect_app[$i]}" ]]; then
    echo "checksum mismatch at line $((i + 1)): got ${LINES[$i]}, want ${expect_app[$i]}" >&2
    exit 1
  fi
done
echo "checksums ok (${#expect_app[@]} scenarios)"

echo "== timing + peak RSS (app, RUNS=$RUNS) =="
stats="$(bench_measure_runs "$BIN" "$RUNS")"
bench_print_stats "bench_app" "$stats"

if [[ "${SKIP_STR:-0}" != "1" ]]; then
  echo "building examples/bench/bench_str.lm → $BIN_STR"
  "$LUMIA" build --release examples/bench/bench_str.lm -o "$BIN_STR"
  echo "== checksums (str) =="
  got_str="$("$BIN_STR")"
  want_str="384000"
  printf '  %-22s %s\n' "str_pipeline(8k):" "$got_str"
  if [[ "$got_str" != "$want_str" ]]; then
    echo "checksum mismatch: got $got_str, want $want_str" >&2
    exit 1
  fi
  echo "checksums ok (1 scenario)"
  echo "== timing + peak RSS (str, RUNS=$RUNS) =="
  stats_str="$(bench_measure_runs "$BIN_STR" "$RUNS")"
  bench_print_stats "bench_str" "$stats_str"
fi

echo "bench_app: OK"

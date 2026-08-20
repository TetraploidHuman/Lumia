#!/usr/bin/env bash
# GC / heap soak microbench (Release).
# Scenarios — see examples/bench/bench_gc.lm + bench_gc_retain.lm:
#   list_churn, map_churn, nest_retain, cow_traffic
#
# Env:
#   RUNS=7               # wall-clock samples (default 7)
#   SKIP_RETAIN=1        # skip nest/COW binary
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
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_gc"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/bench_gc"
BIN_R="$OUT_DIR/bench_gc_retain"
RUNS="${RUNS:-7}"

echo "======== lumia bench_gc (Release) ========"
echo "building examples/bench/bench_gc.lm → $BIN"
"$LUMIA" build --release examples/bench/bench_gc.lm -o "$BIN"

expect_gc=(
  32380020
  105033750
)
labels_gc=(
  "list_churn(8k×32)"
  "map_churn(4k×6)"
)

echo "== checksums (churn) =="
mapfile -t LINES < <("$BIN")
if [[ ${#LINES[@]} -ne ${#expect_gc[@]} ]]; then
  echo "expected ${#expect_gc[@]} checksum lines, got ${#LINES[@]}" >&2
  printf '%s\n' "${LINES[@]}" >&2
  exit 1
fi
for i in "${!expect_gc[@]}"; do
  printf '  %-22s %s\n' "${labels_gc[$i]}:" "${LINES[$i]}"
  if [[ "${LINES[$i]}" != "${expect_gc[$i]}" ]]; then
    echo "checksum mismatch at line $((i + 1)): got ${LINES[$i]}, want ${expect_gc[$i]}" >&2
    exit 1
  fi
done
echo "checksums ok (${#expect_gc[@]} scenarios)"

echo "== timing + peak RSS (churn, RUNS=$RUNS) =="
stats="$(bench_measure_runs "$BIN" "$RUNS")"
bench_print_stats "gc_churn" "$stats"

if [[ "${SKIP_RETAIN:-0}" != "1" ]]; then
  echo "building examples/bench/bench_gc_retain.lm → $BIN_R"
  "$LUMIA" build --release examples/bench/bench_gc_retain.lm -o "$BIN_R"
  expect_r=(
    231840
    100050755
  )
  labels_r=(
    "nest_retain(120×16)"
    "cow_traffic(100k)"
  )
  echo "== checksums (retain) =="
  mapfile -t LINES_R < <("$BIN_R")
  if [[ ${#LINES_R[@]} -ne ${#expect_r[@]} ]]; then
    echo "expected ${#expect_r[@]} checksum lines, got ${#LINES_R[@]}" >&2
    printf '%s\n' "${LINES_R[@]}" >&2
    exit 1
  fi
  for i in "${!expect_r[@]}"; do
    printf '  %-22s %s\n' "${labels_r[$i]}:" "${LINES_R[$i]}"
    if [[ "${LINES_R[$i]}" != "${expect_r[$i]}" ]]; then
      echo "checksum mismatch at line $((i + 1)): got ${LINES_R[$i]}, want ${expect_r[$i]}" >&2
      exit 1
    fi
  done
  echo "checksums ok (${#expect_r[@]} scenarios)"
  echo "== timing + peak RSS (retain, RUNS=$RUNS) =="
  stats_r="$(bench_measure_runs "$BIN_R" "$RUNS")"
  bench_print_stats "gc_retain" "$stats_r"
fi

echo "bench_gc: OK"

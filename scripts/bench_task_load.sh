#!/usr/bin/env bash
# Production-scale Task/Channel load (Release).
# Scenarios — see examples/bench/bench_task_load.lm:
#   fan_in_wide(2048), join_tree(32×16), pipeline(4000), pingpong_long(4000)
#
# Env:
#   RUNS=5               # samples per WORKERS config (default 5)
#   SKIP_VERIFY=1        # skip checksum cross-check
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
SRC=examples/bench/bench_task_load.lm
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_task_load"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/bench_task_load"
RUNS="${RUNS:-5}"

echo "======== lumia bench_task_load (Release) ========"
echo "building $SRC → $BIN"
"$LUMIA" build --release "$SRC" -o "$BIN"

expect_out() {
  printf '%s\n' "2098176" "8452352" "16008000" "8002000"
}

verify_one() {
  local workers=$1
  local got
  got="$(LUMIA_SCHED_WORKERS="$workers" LUMIA_SCHED_IO="$workers" "$BIN")"
  local want
  want="$(expect_out)"
  if [[ "$got" != "$want" ]]; then
    echo "FAIL checksum WORKERS=$workers" >&2
    echo " got:"$'\n'"$got" >&2
    echo " want:"$'\n'"$want" >&2
    return 1
  fi
  echo "OK checksum WORKERS=$workers"
}

if [[ "${SKIP_VERIFY:-0}" != "1" ]]; then
  verify_one 0
  verify_one 1
  verify_one 2
fi

for workers in 0 1 2; do
  samples=""
  for ((i = 1; i <= RUNS; i++)); do
    samples+="$(
      LUMIA_SCHED_WORKERS="$workers" LUMIA_SCHED_IO="$workers" \
        bench_measure "$BIN"
    )"$'\n'
  done
  stats="$(printf '%s' "$samples" | bench_measure_stats)"
  bench_print_stats "task_load_w${workers}" "$stats"
done

echo "bench_task_load: OK"

#!/usr/bin/env bash
# Task / Channel coroutine microbench (Release).
#
# Scenarios in examples/bench_task.lm:
#   fan_in(256), join_many(256), pingpong(500)
# Checksums must match under LUMIA_SCHED_WORKERS=0|1|2.
#
# Env:
#   RUNS=7                 # samples per config (default 7)
#   BENCH_SHIELD=0         # default off
#   SKIP_VERIFY=1          # skip checksum cross-check
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
SRC=examples/bench_task.lm
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_task"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/bench_task"
RUNS="${RUNS:-7}"

echo "======== lumia bench_task (Release) ========"
echo "building $SRC → $BIN"
"$LUMIA" build --release "$SRC" -o "$BIN"

expect_out() {
  # fan_in 256 = 32896; join_many 256 = 32896; pingpong 500 = 125250
  printf '%s\n' "32896" "32896" "125250"
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
  bench_print_stats "task_w${workers}" "$stats"
done

echo "======== RT stress (Release lib tests) ========"
RUST_TEST_THREADS=1 cargo test -q -p lumia_rt --release --lib task::stress:: -- --nocapture 2>&1 \
  | tail -30

echo "bench_task: OK"

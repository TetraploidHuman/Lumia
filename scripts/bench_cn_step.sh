#!/usr/bin/env bash
# Full CogniNucleus-ish step microbench (Release).
#
# Extends the hot GEMV/Hebbian path with sensory fill/scale/add, gate mul, and
# weight decay. Compares `std.linalg` vs nested loops under `--no-dense-f64-sr`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cn_step"
mkdir -p "$OUT_DIR"
RUNS="${RUNS:-5}"

echo "== build =="
"$LUMIA" build --release examples/bench_cn_step_kernel.lm -o "$OUT_DIR/kernel"
"$LUMIA" build --release --no-dense-f64-sr examples/bench_cn_step_naive.lm -o "$OUT_DIR/naive"

echo "== checksum parity =="
k_out="$("$OUT_DIR/kernel")"
n_out="$("$OUT_DIR/naive")"
bench_checksum_parity "$k_out" "$n_out" "naive (--no-dense-f64-sr)"

echo "== wall time + peak RSS  RUNS=$RUNS  STEPS=100000 =="
k_stats="$(bench_measure_runs "$OUT_DIR/kernel")"
n_stats="$(bench_measure_runs "$OUT_DIR/naive")"
bench_print_stats "kernel" "$k_stats"
bench_print_stats "naive_nosr" "$n_stats"
bench_print_speedup_pair "$k_stats" "$n_stats" "naive_nosr"
echo "OK"

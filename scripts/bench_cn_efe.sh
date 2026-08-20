#!/usr/bin/env bash
# CogniNucleus EFE action-scores microbench (Release).
#
# Compares pure-Lumia imagine+G(a) loops vs fused `lumia_efe_action_scores`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cn_efe"
mkdir -p "$OUT_DIR"
RUNS="${RUNS:-5}"

echo "== build =="
"$LUMIA" build --release examples/bench/bench_cn_efe_kernel.lm -o "$OUT_DIR/kernel"
"$LUMIA" build --release examples/bench/bench_cn_efe_naive.lm -o "$OUT_DIR/naive"

echo "== checksum parity =="
k_out="$("$OUT_DIR/kernel")"
n_out="$("$OUT_DIR/naive")"
bench_checksum_parity "$k_out" "$n_out" "naive"

echo "== wall time + peak RSS  RUNS=$RUNS  STEPS=50000 HORIZON=2 =="
k_stats="$(bench_measure_runs "$OUT_DIR/kernel")"
n_stats="$(bench_measure_runs "$OUT_DIR/naive")"
bench_print_stats "kernel" "$k_stats"
bench_print_stats "naive" "$n_stats"
bench_print_speedup_pair "$k_stats" "$n_stats" "naive"
echo "OK"

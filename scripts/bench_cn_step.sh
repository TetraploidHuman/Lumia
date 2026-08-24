#!/usr/bin/env bash
# Full CogniNucleus-ish step microbench (Release).
#
# Extends the hot GEMV/Hebbian path with sensory fill/scale/add, gate mul, and
# weight decay — patterns that should auto-SR to lumi_f64_{fill,scale,mul,add}.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumi --release
LUMI="$ROOT/target/release/lumi"
OUT_DIR="${TMPDIR:-/tmp}/lumi_bench_cn_step"
mkdir -p "$OUT_DIR"
RUNS="${RUNS:-5}"

echo "== build =="
"$LUMI" build --release examples/bench_cn_step_kernel.lm -o "$OUT_DIR/kernel"
"$LUMI" build --release examples/bench_cn_step_naive.lm -o "$OUT_DIR/naive"

echo "== checksum parity =="
k_out="$("$OUT_DIR/kernel")"
n_out="$("$OUT_DIR/naive")"
echo "kernel:"
echo "$k_out"
echo "naive:"
echo "$n_out"
if [[ "$k_out" != "$n_out" ]]; then
  echo "ERROR: checksum mismatch" >&2
  exit 1
fi

measure_bin() {
  local bin=$1
  local samples="" i
  for ((i = 0; i < RUNS; i++)); do
    samples+="$(bench_measure "$bin")"$'\n'
  done
  printf '%s' "$samples" | bench_measure_stats
}

echo "== wall time + peak RSS  RUNS=$RUNS  STEPS=100000 =="
k_stats="$(measure_bin "$OUT_DIR/kernel")"
n_stats="$(measure_bin "$OUT_DIR/naive")"
bench_print_stats "kernel" "$k_stats"
bench_print_stats "naive" "$n_stats"
python3 - "$k_stats" "$n_stats" <<'PY'
import sys
k = sys.argv[1].split()
n = sys.argv[2].split()
kt, nt = float(k[1]), float(n[1])
kr, nr = float(k[4]), float(n[4])
print(f"speedup  {nt/kt:.2f}x  (naive_med_time / kernel_med_time)")
print(f"rss_ratio {nr/kr:.2f}x  (naive_med_rss / kernel_med_rss)")
PY
echo "OK"

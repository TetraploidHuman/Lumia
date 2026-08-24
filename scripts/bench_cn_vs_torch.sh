#!/usr/bin/env bash
# Compare Lumi CN triad-forward microbench vs CogniNucleus PyTorch CPU agent.
#
# Lumi: examples/bench_cn_forward_{kernel,naive}.lm (dense skeleton).
# Torch: FreeEnergyAgent triad+EFE (hip/amy off), default strict_pe+cluster.
#   LEGACY=1  → pass --legacy to the Python harness
#   BOTH=1    → time strict and legacy
#
# Env:
#   RUNS=3 STEPS=20000
#   COGNINUCLEUS_ROOT=...   # default: ../CogniNucleus next to Lumi
#   TORCH_NUM_THREADS=1
#   CN_PYTHON=...           # default: $COGNINUCLEUS_ROOT/.venv/bin/python
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
CN_ROOT="${COGNINUCLEUS_ROOT:-$(cd "$ROOT/../CogniNucleus" && pwd)}"
CN_PYTHON="${CN_PYTHON:-$CN_ROOT/.venv/bin/python}"
RUNS="${RUNS:-3}"
STEPS="${STEPS:-20000}"
export TORCH_NUM_THREADS="${TORCH_NUM_THREADS:-1}"
export COGNINUCLEUS_ROOT="$CN_ROOT"

if [[ ! -x "$CN_PYTHON" ]]; then
  echo "ERROR: CogniNucleus python not found: $CN_PYTHON" >&2
  exit 1
fi

cargo build -q -p lumi --release
LUMI="$ROOT/target/release/lumi"
OUT_DIR="${TMPDIR:-/tmp}/lumi_bench_cn_vs_torch"
mkdir -p "$OUT_DIR"

echo "======== Lumi vs CogniNucleus (CPU) ========"
echo "STEPS=$STEPS  RUNS=$RUNS  TORCH_NUM_THREADS=$TORCH_NUM_THREADS"
echo "CN_ROOT=$CN_ROOT"
echo

echo "== Lumi build =="
"$LUMI" build --release examples/bench_cn_forward_kernel.lm -o "$OUT_DIR/lumi_kernel"
"$LUMI" build --release examples/bench_cn_forward_naive.lm -o "$OUT_DIR/lumi_naive"

measure_bin() {
  local bin=$1
  local samples="" i
  for ((i = 0; i < RUNS; i++)); do
    samples+="$(bench_measure "$bin")"$'\n'
  done
  printf '%s' "$samples" | bench_measure_stats
}

echo "== Lumi wall time =="
k_stats="$(measure_bin "$OUT_DIR/lumi_kernel")"
n_stats="$(measure_bin "$OUT_DIR/lumi_naive")"
bench_print_stats "lumi_kernel" "$k_stats"
bench_print_stats "lumi_naive" "$n_stats"

PY_FLAGS=()
if [[ "${BOTH:-0}" == "1" ]]; then
  PY_FLAGS+=(--both)
elif [[ "${LEGACY:-0}" == "1" ]]; then
  PY_FLAGS+=(--legacy)
fi

echo
echo "== PyTorch CogniNucleus =="
torch_out="$("$CN_PYTHON" "$ROOT/scripts/bench_cn_vs_torch.py" --steps "$STEPS" --runs "$RUNS" "${PY_FLAGS[@]+"${PY_FLAGS[@]}"}")"
printf '%s\n' "$torch_out"

echo
echo "== compare (medians) =="
python3 - "$k_stats" "$n_stats" "$STEPS" "$torch_out" <<'PY'
import sys
k = sys.argv[1].split()
n = sys.argv[2].split()
steps = int(sys.argv[3])
torch_out = sys.argv[4]
kt, nt = float(k[1]), float(n[1])
kus, nus = kt / steps * 1e6, nt / steps * 1e6
print(f"lumi_kernel        {kt:.4f}s  ({kus:.1f} µs/step)")
print(f"lumi_naive         {nt:.4f}s  ({nus:.1f} µs/step)")
for line in torch_out.splitlines():
    if "_US=" in line and line.startswith("TORCH_"):
        key, us = line.split("=", 1)
        usf = float(us)
        print(f"{key[6:].lower():20s} ({usf:.1f} µs/step)  lumi_kernel is {usf/kus:.0f}× faster")
print()
print("Caveat: Lumi bench is a dense triad+EFE+Hebbian skeleton (same dims),")
print("not a full FreeEnergyAgent feature port. Default Torch config is")
print("strict_pe+cluster_rates (CN defaults); LEGACY=1 / BOTH=1 for older PE.")
PY
echo "OK"

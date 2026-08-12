#!/usr/bin/env bash
# Compare Lumia CN triad-forward microbench vs CogniNucleus PyTorch CPU agent.
#
# Lumia: examples/bench_cn_forward_{kernel,naive}.lm (dense skeleton).
# Torch: FreeEnergyAgent triad+EFE (hip/amy off), forward and forward+learn.
#
# Env:
#   RUNS=3 STEPS=20000
#   COGNINUCLEUS_ROOT=...   # default: ../CogniNucleus next to Lumia
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

cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cn_vs_torch"
mkdir -p "$OUT_DIR"

echo "======== Lumia vs CogniNucleus (CPU) ========"
echo "STEPS=$STEPS  RUNS=$RUNS  TORCH_NUM_THREADS=$TORCH_NUM_THREADS"
echo "CN_ROOT=$CN_ROOT"
echo

echo "== Lumia build =="
"$LUMIA" build --release examples/bench_cn_forward_kernel.lm -o "$OUT_DIR/lumia_kernel"
"$LUMIA" build --release examples/bench_cn_forward_naive.lm -o "$OUT_DIR/lumia_naive"

measure_bin() {
  local bin=$1
  local samples="" i
  for ((i = 0; i < RUNS; i++)); do
    samples+="$(bench_measure "$bin")"$'\n'
  done
  printf '%s' "$samples" | bench_measure_stats
}

echo "== Lumia wall time =="
k_stats="$(measure_bin "$OUT_DIR/lumia_kernel")"
n_stats="$(measure_bin "$OUT_DIR/lumia_naive")"
bench_print_stats "lumia_kernel" "$k_stats"
bench_print_stats "lumia_naive" "$n_stats"

echo
echo "== PyTorch CogniNucleus =="
torch_out="$("$CN_PYTHON" "$ROOT/scripts/bench_cn_vs_torch.py" --steps "$STEPS" --runs "$RUNS")"
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
tf = tl = None
for line in torch_out.splitlines():
    if line.startswith("TORCH_FORWARD_US="):
        tf = float(line.split("=", 1)[1])
    elif line.startswith("TORCH_FORWARD_LEARN_US="):
        tl = float(line.split("=", 1)[1])
print(f"lumia_kernel        {kt:.4f}s  ({kus:.1f} µs/step)")
print(f"lumia_naive         {nt:.4f}s  ({nus:.1f} µs/step)")
if tf is not None:
    print(f"torch_forward       ({tf:.1f} µs/step)  lumia_kernel is {tf/kus:.0f}× faster")
if tl is not None:
    print(f"torch_forward+learn ({tl:.1f} µs/step)  lumia_kernel is {tl/kus:.0f}× faster")
print()
print("Caveat: Lumia bench is a dense triad+EFE+Hebbian skeleton (same dims),")
print("not a full FreeEnergyAgent feature port. Numbers show dispatch/overhead gap.")
PY
echo "OK"

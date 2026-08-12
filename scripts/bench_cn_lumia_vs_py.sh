#!/usr/bin/env bash
# Compare CogniNucleusForLumia (native) vs CogniNucleus PyTorch on triad+grid episodes.
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
EPISODES="${EPISODES:-500}"
export TORCH_NUM_THREADS="${TORCH_NUM_THREADS:-1}"
export COGNINUCLEUS_ROOT="$CN_ROOT"

if [[ ! -x "$CN_PYTHON" ]]; then
  echo "ERROR: CogniNucleus python not found: $CN_PYTHON" >&2
  exit 1
fi

cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cn_lumia_vs_py"
mkdir -p "$OUT_DIR"

echo "======== CogniNucleusForLumia vs PyTorch ========"
echo "EPISODES=$EPISODES  RUNS=$RUNS  TORCH_NUM_THREADS=$TORCH_NUM_THREADS"
echo

# Patch episode count into a build-time constant via sed copy if needed:
# bench_episodes.lm hardcodes EPISODES=500; keep in sync with EPISODES env.
if [[ "$EPISODES" != "500" ]]; then
  echo "NOTE: Lumia bench_episodes.lm is fixed at 500; set EPISODES=500" >&2
  EPISODES=500
fi

echo "== Lumia build =="
"$LUMIA" build --release CogniNucleusForLumia/bench_episodes.lm -o "$OUT_DIR/lumia_eps"

measure_bin() {
  local bin=$1
  local samples="" i
  for ((i = 0; i < RUNS; i++)); do
    samples+="$(bench_measure "$bin")"$'\n'
  done
  printf '%s' "$samples" | bench_measure_stats
}

echo "== Lumia wall time =="
# One untimed run for checksum / step count
lumia_out="$("$OUT_DIR/lumia_eps")"
printf '%s\n' "$lumia_out"
k_stats="$(measure_bin "$OUT_DIR/lumia_eps")"
bench_print_stats "lumia" "$k_stats"

echo
echo "== PyTorch CogniNucleus =="
torch_out="$("$CN_PYTHON" "$ROOT/scripts/bench_cn_lumia_vs_py.py" --episodes "$EPISODES" --runs "$RUNS")"
printf '%s\n' "$torch_out"

echo
echo "== compare =="
python3 - "$k_stats" "$lumia_out" "$torch_out" "$EPISODES" <<'PY'
import sys
k = sys.argv[1].split()
lumia_out = sys.argv[2]
torch_out = sys.argv[3]
episodes = int(sys.argv[4])
kt = float(k[1])
lines = lumia_out.strip().splitlines()
lumia_steps = int(lines[1])
kus = kt / lumia_steps * 1e6
tus = None
t_steps = None
for line in torch_out.splitlines():
    if line.startswith("TORCH_EPISODES_US="):
        tus = float(line.split("=", 1)[1])
    elif line.startswith("TORCH_TOTAL_STEPS="):
        t_steps = int(line.split("=", 1)[1])
print(f"episodes            {episodes}")
print(f"lumia_steps         {lumia_steps}  ({kus:.1f} µs/step med wall)")
if t_steps is not None:
    print(f"torch_steps         {t_steps}")
if tus is not None:
    print(f"torch               ({tus:.1f} µs/step med)")
    print(f"speedup             {tus/kus:.1f}×  (torch_us / lumia_us)")
print()
print("Caveat: same dims/config family, not bit-identical trajectories;")
print("Lumia is a dense triad port; Torch FreeEnergyAgent has more Python glue.")
PY
echo "OK"

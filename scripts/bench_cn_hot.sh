#!/usr/bin/env bash
# CogniNucleus-shaped dense-float hot-path microbench (Release).
#
# Compares nested Lumia List[Float] loops vs std.linalg kernels (same fingerprints).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

stats_min_med_max() {
  awk '
    {
      n = split($0, a, /[[:space:]]+/)
      m = 0
      for (i = 1; i <= n; i++) if (a[i] != "") b[++m] = a[i] + 0
    }
    END {
      if (m < 1) { print "n/a n/a n/a"; exit }
      for (i = 1; i <= m; i++)
        for (j = i + 1; j <= m; j++)
          if (b[j] < b[i]) { t = b[i]; b[i] = b[j]; b[j] = t }
      min = b[1]; max = b[m]
      if (m % 2 == 1) med = b[int((m + 1) / 2)]
      else med = (b[m / 2] + b[m / 2 + 1]) / 2
      printf "%.4f %.4f %.4f\n", min, med, max
    }'
}

elapsed() {
  # prints elapsed seconds for "$@" to stdout; command stdout discarded
  python3 - "$@" <<'PY'
import subprocess, sys, time
cmd = sys.argv[1:]
t0 = time.perf_counter()
subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)
print(f"{time.perf_counter() - t0:.6f}")
PY
}

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cn_hot"
mkdir -p "$OUT_DIR"
RUNS="${RUNS:-5}"

echo "== build =="
"$LUMIA" build --release examples/bench_cn_hot_kernel.lm -o "$OUT_DIR/kernel"
"$LUMIA" build --release examples/bench_cn_hot_naive.lm -o "$OUT_DIR/naive"

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

time_bin() {
  local bin=$1
  local samples="" i
  for ((i = 0; i < RUNS; i++)); do
    samples+="$(elapsed "$bin") "
  done
  echo "$samples" | stats_min_med_max
}

echo "== wall time (s) min/median/max  RUNS=$RUNS  STEPS=100000 =="
k_stats="$(time_bin "$OUT_DIR/kernel")"
n_stats="$(time_bin "$OUT_DIR/naive")"
echo "kernel  $k_stats"
echo "naive   $n_stats"
python3 - "$k_stats" "$n_stats" <<'PY'
import sys
k = float(sys.argv[1].split()[1])
n = float(sys.argv[2].split()[1])
print(f"speedup  {n/k:.2f}x  (naive_median / kernel_median)")
PY

if python3 -c 'import torch' 2>/dev/null; then
  echo "== torch reference =="
  python3 - "$RUNS" <<'PY'
import sys, time
import torch
RUNS = int(sys.argv[1])
VIS, PFC, STEPS = 16, 32, 100_000
LR, CLIP, EPS = 0.05, 10.0, 1e-3

def run():
    mu_vis = torch.arange(VIS, dtype=torch.float64) * 0.01
    mu_pfc = torch.arange(PFC, dtype=torch.float64) * 0.001
    i = torch.arange(PFC, dtype=torch.float64).unsqueeze(1)
    j = torch.arange(VIS, dtype=torch.float64).unsqueeze(0)
    w_vp = (i + j) * 0.001
    enc = torch.eye(PFC, dtype=torch.float64)
    pred = torch.eye(PFC, dtype=torch.float64)
    for _ in range(STEPS):
        drive = w_vp @ mu_vis
        pred_buf = pred.T @ mu_pfc
        err = drive - pred_buf
        delta = enc.T @ err
        mu_pfc = (mu_pfc + LR * delta).clamp(-CLIP, CLIP)
        u = mu_vis / (mu_vis.norm() + EPS)
        v = err / (err.norm() + EPS)
        w_vp = w_vp + LR * torch.outer(v, u)
        mu_vis = (mu_vis + 0.0001 * u).clamp(-CLIP, CLIP)
    def checksum(x):
        return int(torch.floor(x.sum() * 1000).item())
    return checksum(mu_pfc), checksum(w_vp.reshape(-1))

print("torch checksums:")
print(run()[0]); print(run()[1])
samples = []
for _ in range(RUNS):
    t0 = time.perf_counter(); run(); samples.append(time.perf_counter() - t0)
samples.sort()
print(f"torch   {samples[0]:.4f} {samples[len(samples)//2]:.4f} {samples[-1]:.4f}")
PY
else
  echo "(skip torch)"
fi
echo "OK"

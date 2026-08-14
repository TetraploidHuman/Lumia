#!/usr/bin/env bash
# CogniNucleus-shaped dense-float hot-path microbench (Release).
#
# Compares:
#   kernel  — `std.linalg` → `lumia_f64_*`
#   naive   — nested List[Float] loops with `--no-dense-f64-sr` (scalar get/set)
#
# With SR on, naive loops rewrite to the same RT calls as kernel (parity ≈ 1.0×);
# this bench measures the real SR win vs a scalar baseline.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cn_hot"
mkdir -p "$OUT_DIR"
RUNS="${RUNS:-5}"

echo "== build =="
"$LUMIA" build --release examples/bench_cn_hot_kernel.lm -o "$OUT_DIR/kernel"
"$LUMIA" build --release --no-dense-f64-sr examples/bench_cn_hot_naive.lm -o "$OUT_DIR/naive"

echo "== checksum parity =="
k_out="$("$OUT_DIR/kernel")"
n_out="$("$OUT_DIR/naive")"
echo "kernel:"
echo "$k_out"
echo "naive (--no-dense-f64-sr):"
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
bench_print_stats "naive_nosr" "$n_stats"
python3 - "$k_stats" "$n_stats" <<'PY'
import sys
k = sys.argv[1].split()
n = sys.argv[2].split()
kt, nt = float(k[1]), float(n[1])
kr, nr = float(k[4]), float(n[4])
print(f"speedup  {nt/kt:.2f}x  (naive_nosr_med_time / kernel_med_time)")
print(f"rss_ratio {nr/kr:.2f}x  (naive_nosr_med_rss / kernel_med_rss)")
PY

if python3 -c 'import torch' 2>/dev/null; then
  echo "== torch reference =="
  python3 - "$RUNS" <<'PY'
import sys, time, resource
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
times, rss = [], []
for _ in range(RUNS):
    # self RSS before/after is noisy for in-process; report process maxrss delta loosely
    before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    t0 = time.perf_counter()
    run()
    times.append(time.perf_counter() - t0)
    after = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    rss.append(max(before, after))
times.sort(); rss.sort()
print(f"torch   time {times[0]:.4f} {times[len(times)//2]:.4f} {times[-1]:.4f}")
print(f"torch   rss  {rss[0]:.0f} {rss[len(rss)//2]:.0f} {rss[-1]:.0f}")
PY
else
  echo "(skip torch)"
fi
echo "OK"

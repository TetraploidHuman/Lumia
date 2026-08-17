#!/usr/bin/env bash
# CPU compute-intensive microbench (Release).
# Kernels / scenarios — see examples/bench_cpu.lm:
#   primes, matmul, Mandelbrot, Collatz dense+strided, fib, poly,
#   gcd, divisorSum, productRem, floatOrbit, rangeFold, memTraffic
#
# Affinity / shield (see scripts/bench_affinity.sh, scripts/bench_shield.sh):
#   BENCH_CORES=4|2       # physical cores to reserve (default 4 → last N)
#   BENCH_CPUS=14,15      # explicit primary logical CPUs
#   BENCH_SHIELD=auto|1|0 # exclusive cpuset+performance (needs sudo; default auto)
#   BENCH_SUDO_PASS=...   # optional; enables non-interactive `sudo -S` for shield
#   RUNS=11               # wall-clock samples (default 11; odd ⇒ clean median)
#   COMPARE_DEBUG=1       # also time Debug and check checksum parity
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_affinity.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

sudo_shield() {
  # Run bench_shield.sh as root. Prefer cached/NOPASSWD; else BENCH_SUDO_PASS via -S.
  local shield="$ROOT/scripts/bench_shield.sh"
  if sudo -n true 2>/dev/null; then
    sudo -n "$shield" "$@"
  elif [[ -n "${BENCH_SUDO_PASS:-}" ]]; then
    printf '%s\n' "$BENCH_SUDO_PASS" | sudo -S -p '' "$shield" "$@"
  else
    sudo "$shield" "$@"
  fi
}

# Sort numeric samples → print min median max (space-separated on stdin).
# Prefer bench_measure_stats (time+rss); kept for any external callers.
stats_min_med_max() {
  awk '
    {
      n = split($0, a, /[[:space:]]+/)
      m = 0
      for (i = 1; i <= n; i++) if (a[i] != "") b[++m] = a[i] + 0
      if (m < 1) { print "n/a n/a n/a"; exit }
      for (i = 1; i <= m; i++)
        for (j = i + 1; j <= m; j++)
          if (b[j] < b[i]) { t = b[i]; b[i] = b[j]; b[j] = t }
      min = b[1]; max = b[m]
      if (m % 2 == 1) med = b[int((m + 1) / 2)]
      else med = (b[m / 2] + b[m / 2 + 1]) / 2
      printf "%.3f %.3f %.3f\n", min, med, max
    }'
}

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
SRC=examples/bench_cpu.lm
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cpu"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/bench_cpu"
RUNS="${RUNS:-11}"
BENCH_SHIELD="${BENCH_SHIELD:-auto}"

BENCH_CPU_LIST="$(bench_pick_cpus)"
export BENCH_CPUS="$BENCH_CPU_LIST"
SIB_HINT="$(bench_sibling_hint "$BENCH_CPU_LIST" || true)"

use_shield=0
case "$BENCH_SHIELD" in
  1 | yes | true)
    use_shield=1
    ;;
  0 | no | false)
    use_shield=0
    ;;
  auto)
    if [[ -x "$ROOT/scripts/bench_shield.sh" ]] && {
      sudo -n true 2>/dev/null || [[ -n "${BENCH_SUDO_PASS:-}" ]]
    }; then
      use_shield=1
    fi
    ;;
esac

echo "== affinity =="
printf '  primary CPUs: %s (BENCH_CORES=%s)\n' "$BENCH_CPU_LIST" "${BENCH_CORES:-4}"
if [[ -n "${SIB_HINT:-}" ]]; then
  printf '  SMT siblings:  %s\n' "$SIB_HINT"
fi
if [[ "$use_shield" -eq 1 ]]; then
  printf '  mode: exclusive cpuset shield + performance governor (sudo)\n'
else
  printf '  mode: taskset pin only (set BENCH_SHIELD=1 after sudo -v for full shield)\n'
fi

echo "== build Release =="
"$LUMIA" build --release "$SRC" -o "$BIN"

echo "== checksums (single run) =="
mapfile -t LINES < <(bench_run "$BIN")
expect=(
  63951
  1998964270721
  2872327
  352279148
  142794532
  102334155
  720427763375
  9122320
  197458334
  405134546788
  3920082
  25014941572
  860371869
)
labels=(
  "primes(800k)"
  "matmul(2000)"
  "mandelbrot(450)"
  "collatzTotal(2.5M)"
  "collatzStrided(3M/3)"
  "fib(40)"
  "poly(12k)"
  "gcd(1400)"
  "divisorSum(12M)"
  "productRem(9k)"
  "floatOrbit(100k×50)"
  "rangeFold(5M)"
  "memTraffic(1.5M)"
)
if [[ ${#LINES[@]} -ne ${#expect[@]} ]]; then
  echo "expected ${#expect[@]} checksum lines, got ${#LINES[@]}" >&2
  printf '%s\n' "${LINES[@]}" >&2
  exit 1
fi
for i in "${!expect[@]}"; do
  printf '  %-22s %s\n' "${labels[$i]}:" "${LINES[$i]}"
  if [[ "${LINES[$i]}" != "${expect[$i]}" ]]; then
    echo "checksum mismatch at line $((i + 1)): got ${LINES[$i]}, want ${expect[$i]}" >&2
    echo "(update expect[] in scripts/bench_cpu.sh if the workload intentionally changed)" >&2
    exit 1
  fi
done
echo "checksums ok (${#expect[@]} scenarios)"

# Collect RUNS "time rss_kb" samples for bin on cpus.
collect_samples() {
  local bin=$1
  local runs=$2
  local cpus=$3
  local i
  for ((i = 1; i <= runs; i++)); do
    if command -v taskset >/dev/null 2>&1; then
      bench_measure taskset -c "$cpus" "$bin"
    else
      bench_measure "$bin"
    fi
  done
}

echo "== timing + peak RSS (${RUNS} samples, whole suite) =="
if [[ "$use_shield" -eq 1 ]]; then
  chmod +x "$ROOT/scripts/bench_shield.sh"
  samples="$(
    sudo_shield --cpus "$BENCH_CPU_LIST" -- \
      bash -c '
        # shellcheck disable=SC1091
        # bash -c: first arg after the script is $0.
        source "$0"
        bin=$1; runs=$2; cpus=$3
        for ((i=1;i<=runs;i++)); do
          if command -v taskset >/dev/null 2>&1; then
            bench_measure taskset -c "$cpus" "$bin"
          else
            bench_measure "$bin"
          fi
        done
      ' \
      "$ROOT/scripts/bench_measure.sh" "$BIN" "$RUNS" "$BENCH_CPU_LIST"
  )"
else
  samples="$(collect_samples "$BIN" "$RUNS" "$BENCH_CPU_LIST")"
fi
stats="$(printf '%s\n' "$samples" | bench_measure_stats)"
bench_print_stats "bench_cpu" "$stats"
printf '(%s scenarios: primes + matmul + mandel + collatz×2 + fib + poly + gcd + div + prodRem + floatOrbit + rangeFold + memTraffic)\n' "${#expect[@]}"

# Optional: Debug vs Release contrast (same source).
if [[ "${COMPARE_DEBUG:-0}" == "1" ]]; then
  DBG="$OUT_DIR/bench_cpu_debug"
  echo "== build Debug =="
  "$LUMIA" build "$SRC" -o "$DBG"
  out_d="$(bench_run "$DBG")"
  out_r="$(printf '%s\n' "${LINES[@]}")"
  if [[ "$out_d" != "$out_r" ]]; then
    echo "Debug/Release checksum mismatch" >&2
    exit 1
  fi
  if [[ "$use_shield" -eq 1 ]]; then
    samples_d="$(
      sudo_shield --cpus "$BENCH_CPU_LIST" -- \
        bash -c '
          # shellcheck disable=SC1091
          source "$0"
          bin=$1; runs=$2; cpus=$3
          for ((i=1;i<=runs;i++)); do
            if command -v taskset >/dev/null 2>&1; then
              bench_measure taskset -c "$cpus" "$bin"
            else
              bench_measure "$bin"
            fi
          done
        ' \
        "$ROOT/scripts/bench_measure.sh" "$DBG" "$RUNS" "$BENCH_CPU_LIST"
    )"
  else
    samples_d="$(collect_samples "$DBG" "$RUNS" "$BENCH_CPU_LIST")"
  fi
  stats_d="$(printf '%s\n' "$samples_d" | bench_measure_stats)"
  bench_print_stats "cpu_debug" "$stats_d"
  read -r _ t_med _ _ r_med _ <<<"$stats"
  read -r _ d_med _ _ dr_med _ <<<"$stats_d"
  awk -v d="$d_med" -v r="$t_med" 'BEGIN{
    if (r+0 <= 0) { print "speedup (median Debug/Release time): n/a"; exit }
    printf "speedup (median Debug / Release time): %.2fx\n", d/r
  }'
  awk -v d="$dr_med" -v r="$r_med" 'BEGIN{
    if (r+0 <= 0) { print "rss_ratio (median Debug/Release): n/a"; exit }
    printf "rss_ratio (median Debug / Release): %.2fx\n", d/r
  }'
fi

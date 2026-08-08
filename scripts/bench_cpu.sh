#!/usr/bin/env bash
# CPU compute-intensive microbench (Release).
# Kernels: primes, matmul, Mandelbrot, Collatz, naive fib — see examples/bench_cpu.lm.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

cd "$ROOT"
cargo build -q -p lumia
LUMIA="$ROOT/target/debug/lumia"
SRC=examples/bench_cpu.lm
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_cpu"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/bench_cpu"
RUNS="${RUNS:-3}"

echo "== build Release =="
"$LUMIA" build --release "$SRC" -o "$BIN"

echo "== checksums (single run) =="
mapfile -t LINES < <("$BIN")
if [[ ${#LINES[@]} -ne 5 ]]; then
  echo "expected 5 checksum lines, got ${#LINES[@]}" >&2
  printf '%s\n' "${LINES[@]}" >&2
  exit 1
fi
printf '  primes:     %s\n' "${LINES[0]}"
printf '  matmul:     %s\n' "${LINES[1]}"
printf '  mandelbrot: %s\n' "${LINES[2]}"
printf '  collatz:    %s\n' "${LINES[3]}"
printf '  fib(37):    %s\n' "${LINES[4]}"

# Known fingerprints (must stay stable across compiler changes).
# Recompute with: lumia build --release examples/bench_cpu.lm && run once.
expect=(7837 12803971115 856770 29265567 24157817)
for i in 0 1 2 3 4; do
  if [[ "${LINES[$i]}" != "${expect[$i]}" ]]; then
    echo "checksum mismatch at line $((i + 1)): got ${LINES[$i]}, want ${expect[$i]}" >&2
    echo "(update expect[] in scripts/bench_cpu.sh if the workload intentionally changed)" >&2
    exit 1
  fi
done
echo "checksums ok"

best_of() {
  local bin=$1
  local best=999999
  local i t
  for ((i = 1; i <= RUNS; i++)); do
    t="$(TIMEFORMAT='%R'; { time "$bin" >/dev/null; } 2>&1)"
    best="$(awk -v a="$best" -v b="$t" 'BEGIN{ if (b+0 < a+0) print b; else print a }')"
  done
  echo "$best"
}

echo "== timing (best of ${RUNS} wall-clock seconds, whole suite) =="
t="$(best_of "$BIN")"
printf 'bench_cpu Release: %ss\n' "$t"
printf '(primes + matmul + mandelbrot + collatz + fib(37))\n'

# Optional: Debug vs Release contrast (same source).
if [[ "${COMPARE_DEBUG:-0}" == "1" ]]; then
  DBG="$OUT_DIR/bench_cpu_debug"
  echo "== build Debug =="
  "$LUMIA" build "$SRC" -o "$DBG"
  out_d="$("$DBG")"
  out_r="$(printf '%s\n' "${LINES[@]}")"
  if [[ "$out_d" != "$out_r" ]]; then
    echo "Debug/Release checksum mismatch" >&2
    exit 1
  fi
  t_d="$(best_of "$DBG")"
  printf 'bench_cpu Debug:   %ss\n' "$t_d"
  awk -v d="$t_d" -v r="$t" 'BEGIN{
    if (r+0 <= 0) { print "speedup: n/a"; exit }
    printf "speedup (Debug / Release): %.2fx\n", d/r
  }'
fi

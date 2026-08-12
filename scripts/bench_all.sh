#!/usr/bin/env bash
# Run the full Release microbench suite (checksum + wall time + peak RSS).
#
# Covers prior CPU / Memo benches as well as the CogniNucleus dense-float path,
# so dense-float work cannot silently regress older kernels.
#
# Env:
#   RUNS=5              # samples per timed binary (cn_hot / memo); cpu uses its own default
#   BENCH_CPU_RUNS=5    # override bench_cpu.sh RUNS (default 5 here for quicker gates)
#   BENCH_SHIELD=0      # default off in the aggregate gate (no sudo)
#   SKIP_CPU=1 SKIP_MEMO=1 SKIP_CN=1 SKIP_CN_STEP=1
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

cd "$ROOT"
export BENCH_SHIELD="${BENCH_SHIELD:-0}"
CPU_RUNS="${BENCH_CPU_RUNS:-${RUNS:-5}}"
CN_RUNS="${RUNS:-5}"

echo "======== lumia bench_all (Release) ========"
echo "RUNS(cn/memo)=$CN_RUNS  BENCH_CPU_RUNS=$CPU_RUNS  BENCH_SHIELD=$BENCH_SHIELD"
echo

fail=0

if [[ "${SKIP_CPU:-0}" != "1" ]]; then
  echo "######## bench_cpu ########"
  if RUNS="$CPU_RUNS" bash "$ROOT/scripts/bench_cpu.sh"; then
    echo "OK bench_cpu"
  else
    echo "FAIL bench_cpu" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_MEMO:-0}" != "1" ]]; then
  echo "######## bench_memo ########"
  if bash "$ROOT/scripts/bench_memo.sh"; then
    echo "OK bench_memo"
  else
    echo "FAIL bench_memo" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_CN:-0}" != "1" ]]; then
  echo "######## bench_cn_hot ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_cn_hot.sh"; then
    echo "OK bench_cn_hot"
  else
    echo "FAIL bench_cn_hot" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_CN_STEP:-0}" != "1" ]]; then
  echo "######## bench_cn_step ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_cn_step.sh"; then
    echo "OK bench_cn_step"
  else
    echo "FAIL bench_cn_step" >&2
    fail=1
  fi
  echo
fi

if [[ "$fail" -ne 0 ]]; then
  echo "bench_all: FAILED" >&2
  exit 1
fi
echo "bench_all: OK"

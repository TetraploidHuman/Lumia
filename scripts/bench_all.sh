#!/usr/bin/env bash
# Run the full Release microbench suite (checksum + wall time + peak RSS).
#
# Covers prior CPU / Memo / CN / task benches as well as production-shaped
# app / GC / task-load / compile latency, so dense-float work cannot silently
# regress older kernels and app/GC paths stay in the gate.
#
# Env:
#   RUNS=5              # samples per timed binary (cn_hot / memo); cpu uses its own default
#   BENCH_CPU_RUNS=5    # override bench_cpu.sh RUNS (default 5 here for quicker gates)
#   BENCH_SHIELD=0      # default off in the aggregate gate (no sudo)
#   SKIP_CPU=1 SKIP_MEMO=1 SKIP_CN=1 SKIP_CN_STEP=1 SKIP_CN_EFE=1 SKIP_CN_FUSE=1 SKIP_CN_FORWARD=1 SKIP_CN_STRICT=1
#   SKIP_TASK=1 SKIP_APP=1 SKIP_GC=1 SKIP_TASK_LOAD=1 SKIP_COMPILE=1
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

cd "$ROOT"
# Clang links `liblumia_rt.a` without Rust LTO — ensure the Release staticlib exists
# and is up to date before any example binary is built (avoids stale/unopt RT).
cargo build -q -p lumia_rt --release
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

if [[ "${SKIP_CN_EFE:-0}" != "1" ]]; then
  echo "######## bench_cn_efe ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_cn_efe.sh"; then
    echo "OK bench_cn_efe"
  else
    echo "FAIL bench_cn_efe" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_CN_FUSE:-0}" != "1" ]]; then
  echo "######## bench_cn_fuse ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_cn_fuse.sh"; then
    echo "OK bench_cn_fuse"
  else
    echo "FAIL bench_cn_fuse" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_CN_FORWARD:-0}" != "1" ]]; then
  echo "######## bench_cn_forward ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_cn_forward.sh"; then
    echo "OK bench_cn_forward"
  else
    echo "FAIL bench_cn_forward" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_CN_STRICT:-0}" != "1" ]]; then
  echo "######## bench_cn_strict ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_cn_strict.sh"; then
    echo "OK bench_cn_strict"
  else
    echo "FAIL bench_cn_strict" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_TASK:-0}" != "1" ]]; then
  echo "######## bench_task ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_task.sh"; then
    echo "OK bench_task"
  else
    echo "FAIL bench_task" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_APP:-0}" != "1" ]]; then
  echo "######## bench_app ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_app.sh"; then
    echo "OK bench_app"
  else
    echo "FAIL bench_app" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_GC:-0}" != "1" ]]; then
  echo "######## bench_gc ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_gc.sh"; then
    echo "OK bench_gc"
  else
    echo "FAIL bench_gc" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_TASK_LOAD:-0}" != "1" ]]; then
  echo "######## bench_task_load ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_task_load.sh"; then
    echo "OK bench_task_load"
  else
    echo "FAIL bench_task_load" >&2
    fail=1
  fi
  echo
fi

if [[ "${SKIP_COMPILE:-0}" != "1" ]]; then
  echo "######## bench_compile ########"
  if RUNS="$CN_RUNS" bash "$ROOT/scripts/bench_compile.sh"; then
    echo "OK bench_compile"
  else
    echo "FAIL bench_compile" >&2
    fail=1
  fi
  echo
fi

if [[ "$fail" -ne 0 ]]; then
  echo "bench_all: FAILED" >&2
  exit 1
fi
echo "bench_all: OK"

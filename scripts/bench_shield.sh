#!/usr/bin/env bash
# Exclusive CPU shield for microbenches (requires root / sudo).
#
# Creates a cgroup v2 cpuset partition (isolated) so user.slice/system.slice
# cannot schedule on the reserved physical cores, sets those CPUs to the
# performance governor, runs the command inside the cgroup, then restores.
#
# Usage:
#   sudo ./scripts/bench_shield.sh [--cores N | --cpus LIST] -- <cmd> [args...]
#   BENCH_CORES=2 sudo ./scripts/bench_shield.sh -- ./target/...
#
# Do not put passwords in this script. Use `sudo` / `sudo -S` from the caller.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_affinity.sh"

CG="${LUMIA_BENCH_CGROUP:-/sys/fs/cgroup/lumia_bench}"
CORES_ARG=""
CPUS_ARG=""
CMD=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cores)
      CORES_ARG="$2"
      shift 2
      ;;
    --cpus)
      CPUS_ARG="$2"
      shift 2
      ;;
    --)
      shift
      CMD=("$@")
      break
      ;;
    -h | --help)
      sed -n '2,20p' "$0"
      exit 0
      ;;
    *)
      echo "unknown arg: $1 (use -- before command)" >&2
      exit 2
      ;;
  esac
done

if [[ ${#CMD[@]} -eq 0 ]]; then
  echo "usage: sudo $0 [--cores N | --cpus LIST] -- <cmd>..." >&2
  exit 2
fi

if [[ "$(id -u)" -ne 0 ]]; then
  echo "bench_shield: must run as root (try: sudo $0 ...)" >&2
  exit 1
fi

if [[ -n "$CPUS_ARG" ]]; then
  export BENCH_CPUS="$CPUS_ARG"
elif [[ -n "$CORES_ARG" ]]; then
  export BENCH_CORES="$CORES_ARG"
  unset BENCH_CPUS || true
fi

PRIMARY="$(bench_pick_cpus)"
SIB="$(bench_sibling_hint "$PRIMARY" || true)"
if [[ -n "$SIB" ]]; then
  CPU_LIST="$PRIMARY,$SIB"
else
  CPU_LIST="$PRIMARY"
fi
# Normalize unique sorted list for sysfs.
CPU_LIST="$(echo "$CPU_LIST" | tr ',' '\n' | sort -n | uniq | paste -sd, -)"

RUN_USER="${SUDO_USER:-${USER:-root}}"
declare -A OLD_GOV=()
SHIELD_UP=0

shield_teardown() {
  local cpu
  if [[ "$SHIELD_UP" -eq 1 && -d "$CG" ]]; then
    # Evacuate any remaining procs to root cgroup.
    if [[ -f "$CG/cgroup.procs" ]]; then
      local p
      while read -r p; do
        [[ -n "$p" ]] || continue
        echo "$p" > /sys/fs/cgroup/cgroup.procs 2>/dev/null || true
      done < "$CG/cgroup.procs" || true
    fi
    if [[ -f "$CG/cpuset.cpus.partition" ]]; then
      echo member > "$CG/cpuset.cpus.partition" 2>/dev/null || true
    fi
    rmdir "$CG" 2>/dev/null || true
    SHIELD_UP=0
  fi
  for cpu in "${!OLD_GOV[@]}"; do
    if [[ -f "/sys/devices/system/cpu/cpu${cpu}/cpufreq/scaling_governor" ]]; then
      echo "${OLD_GOV[$cpu]}" \
        >"/sys/devices/system/cpu/cpu${cpu}/cpufreq/scaling_governor" 2>/dev/null || true
    fi
  done
}

shield_setup() {
  local cpu gov_path
  if [[ -d "$CG" ]]; then
    shield_teardown
  fi
  mkdir -p "$CG"
  echo 0 >"$CG/cpuset.mems"
  echo "$CPU_LIST" >"$CG/cpuset.cpus"
  echo isolated >"$CG/cpuset.cpus.partition"
  SHIELD_UP=1

  IFS=',' read -r -a cpu_arr <<<"$CPU_LIST"
  for cpu in "${cpu_arr[@]}"; do
    gov_path="/sys/devices/system/cpu/cpu${cpu}/cpufreq/scaling_governor"
    if [[ -f "$gov_path" ]]; then
      OLD_GOV[$cpu]="$(cat "$gov_path")"
      echo performance >"$gov_path" 2>/dev/null || true
    fi
  done
}

trap shield_teardown EXIT INT TERM

shield_setup

echo "== bench shield ==" >&2
echo "  reserved CPUs: $CPU_LIST (isolated cpuset)" >&2
echo "  user.slice now: $(cat /sys/fs/cgroup/user.slice/cpuset.cpus.effective)" >&2
echo "  run as: $RUN_USER — ${CMD[*]}" >&2

# Start a helper that joins the cgroup *before* dropping privileges / running
# the workload. (If we start in user.slice first, reserved CPUs are already
# gone from that slice and taskset fails.)
set +e
if [[ "$RUN_USER" == root ]]; then
  bash -c '
    set -euo pipefail
    echo $$ >"$1/cgroup.procs"
    shift
    exec "$@"
  ' bash "$CG" "${CMD[@]}" &
else
  bash -c '
    set -euo pipefail
    echo $$ >"$1/cgroup.procs"
    shift
    user=$1
    shift
    exec runuser -u "$user" -- "$@"
  ' bash "$CG" "$RUN_USER" "${CMD[@]}" &
fi
CMD_PID=$!
wait "$CMD_PID"
RC=$?
set -e
exit "$RC"

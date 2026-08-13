# shellcheck shell=bash
# CPU affinity helpers for microbenches (source from other scripts).
#
# Without root we cannot evacuate other processes off these CPUs (no isolcpus /
# cpuset shield). We still pin the workload to the last N *physical* cores
# (primary HT only) so timing is less mixed with CPU0 / scheduler noise.
#
# Env:
#   BENCH_CORES=2|4   — how many physical cores to use (default 4)
#   BENCH_CPUS=12,13  — explicit logical CPU list (overrides BENCH_CORES)

bench_pick_cpus() {
  if [[ -n "${BENCH_CPUS:-}" ]]; then
    echo "$BENCH_CPUS"
    return
  fi
  local n="${BENCH_CORES:-4}"
  local -a primaries=()
  local c id sib first
  for c in /sys/devices/system/cpu/cpu[0-9]*; do
    id="${c##*/cpu}"
    [[ "$id" =~ ^[0-9]+$ ]] || continue
    [[ -f "$c/topology/thread_siblings_list" ]] || continue
    sib="$(cat "$c/topology/thread_siblings_list")"
    first="${sib%%,*}"
    first="${first%%-*}"
    if [[ "$id" == "$first" ]]; then
      primaries+=("$id")
    fi
  done
  if ((${#primaries[@]} == 0)); then
    mapfile -t primaries < <(seq 0 $(($(nproc) - 1)))
  fi
  # Lexical glob order is wrong (cpu10 before cpu2) — sort numerically.
  mapfile -t primaries < <(printf '%s\n' "${primaries[@]}" | sort -n)
  if ((n > ${#primaries[@]})); then
    n=${#primaries[@]}
  fi
  local start=$((${#primaries[@]} - n))
  local -a out=("${primaries[@]:$start:$n}")
  (IFS=','; echo "${out[*]}")
}

# Print sibling HT threads that share the picked physical cores (for awareness).
bench_sibling_hint() {
  local cpus="$1"
  local -a ids
  local id sib
  IFS=',' read -r -a ids <<<"$cpus"
  local -a extras=()
  for id in "${ids[@]}"; do
    [[ -f "/sys/devices/system/cpu/cpu${id}/topology/thread_siblings_list" ]] || continue
    sib="$(cat "/sys/devices/system/cpu/cpu${id}/topology/thread_siblings_list")"
    local part
    IFS=',' read -r -a parts <<<"$sib"
    for part in "${parts[@]}"; do
      if [[ "$part" != "$id" ]]; then
        extras+=("$part")
      fi
    done
  done
  if ((${#extras[@]} > 0)); then
    (IFS=','; echo "${extras[*]}")
  fi
}

bench_run() {
  # Usage: bench_run <cmd> [args...]
  # Pins the command to BENCH_CPUS / BENCH_CORES when `taskset` exists.
  local cpus
  cpus="$(bench_pick_cpus)"
  if command -v taskset >/dev/null 2>&1; then
    taskset -c "$cpus" "$@"
  else
    "$@"
  fi
}

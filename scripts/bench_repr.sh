#!/usr/bin/env bash
# Representation / COW microbenches (Release):
#   take_heap  — materialized HeapList + shared take (Slice COW)
#   take_iota  — virtual range take regression
#   drop_cons  — unique xs=xs.drop (slice_consume)
#   take_app   — shared take then append (bulk materialize)
#   concat_u   — unique xs=xs.concat geometric grow
#   rev_cons   — unique xs=xs.reverse in-place
#   map_lookup — lookup-only large Map
#   map_set_u  — unique hash Map.set in-place
#   sort_cons  — unique xs=xs.sort in-place
#
# Env: RUNS=5 (default), BENCH_CORES / BENCH_CPUS (see bench_affinity.sh)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_affinity.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumi
LUMI="$ROOT/target/debug/lumi"
OUT_DIR="${TMPDIR:-/tmp}/lumi_bench_repr"
mkdir -p "$OUT_DIR"
BIN_TAKE="$OUT_DIR/bench_repr_take"
BIN_TAKE_IOTA="$OUT_DIR/bench_repr_take_iota"
BIN_DROP="$OUT_DIR/bench_repr_drop"
BIN_APPEND="$OUT_DIR/bench_repr_append"
BIN_CONCAT="$OUT_DIR/bench_repr_concat"
BIN_REV="$OUT_DIR/bench_repr_reverse"
BIN_MAP="$OUT_DIR/bench_repr_map"
BIN_MAP_SET="$OUT_DIR/bench_repr_map_set"
BIN_SORT="$OUT_DIR/bench_repr_sort"
RUNS="${RUNS:-5}"

BENCH_CPU_LIST="$(bench_pick_cpus)"
export BENCH_CPUS="$BENCH_CPU_LIST"
echo "== affinity =="
printf '  primary CPUs: %s\n' "$BENCH_CPU_LIST"

echo "== build Release =="
"$LUMI" build --release examples/bench_repr_take.lm -o "$BIN_TAKE"
"$LUMI" build --release examples/bench_repr_take_iota.lm -o "$BIN_TAKE_IOTA"
"$LUMI" build --release examples/bench_repr_drop.lm -o "$BIN_DROP"
"$LUMI" build --release examples/bench_repr_append.lm -o "$BIN_APPEND"
"$LUMI" build --release examples/bench_repr_concat.lm -o "$BIN_CONCAT"
"$LUMI" build --release examples/bench_repr_reverse.lm -o "$BIN_REV"
"$LUMI" build --release examples/bench_repr_map.lm -o "$BIN_MAP"
"$LUMI" build --release examples/bench_repr_map_set.lm -o "$BIN_MAP_SET"
"$LUMI" build --release examples/bench_repr_sort.lm -o "$BIN_SORT"

bench_run_bin() {
  bench_run "$@"
}

echo "== checksums (single run) =="
out_take="$(bench_run_bin "$BIN_TAKE")"
out_take_iota="$(bench_run_bin "$BIN_TAKE_IOTA")"
out_drop="$(bench_run_bin "$BIN_DROP")"
out_append="$(bench_run_bin "$BIN_APPEND")"
out_concat="$(bench_run_bin "$BIN_CONCAT")"
out_rev="$(bench_run_bin "$BIN_REV")"
out_map="$(bench_run_bin "$BIN_MAP")"
out_map_set="$(bench_run_bin "$BIN_MAP_SET")"
out_sort="$(bench_run_bin "$BIN_SORT")"
expect_take=999000000
expect_take_iota=999000000
expect_drop=1600870000
expect_append=79800
expect_concat=100201
expect_rev=8199960
expect_map=666840000
expect_map_set=83584
expect_sort=319960
printf '  take_heap:   %s\n' "$out_take"
printf '  take_iota:   %s\n' "$out_take_iota"
printf '  drop_cons:   %s\n' "$out_drop"
printf '  take_app:    %s\n' "$out_append"
printf '  concat_u:    %s\n' "$out_concat"
printf '  rev_cons:    %s\n' "$out_rev"
printf '  map_lookup:  %s\n' "$out_map"
printf '  map_set_u:   %s\n' "$out_map_set"
printf '  sort_cons:   %s\n' "$out_sort"
if [[ "$out_take" != "$expect_take" ]] \
  || [[ "$out_take_iota" != "$expect_take_iota" ]] \
  || [[ "$out_drop" != "$expect_drop" ]] \
  || [[ "$out_append" != "$expect_append" ]] \
  || [[ "$out_concat" != "$expect_concat" ]] \
  || [[ "$out_rev" != "$expect_rev" ]] \
  || [[ "$out_map" != "$expect_map" ]] \
  || [[ "$out_map_set" != "$expect_map_set" ]] \
  || [[ "$out_sort" != "$expect_sort" ]]; then
  echo "checksum mismatch (update expect_* in scripts/bench_repr.sh if workload changed)" >&2
  echo "  got map_set=$out_map_set sort=$out_sort concat=$out_concat rev=$out_rev" >&2
  exit 1
fi
echo "checksums ok"

measure_n() {
  local bin=$1
  local i
  for ((i = 0; i < RUNS; i++)); do
    if command -v taskset >/dev/null 2>&1; then
      bench_measure taskset -c "$BENCH_CPU_LIST" "$bin"
    else
      bench_measure "$bin"
    fi
  done | bench_measure_stats
}

echo "== timing + peak RSS (RUNS=$RUNS) =="
s_take="$(measure_n "$BIN_TAKE")"
s_take_iota="$(measure_n "$BIN_TAKE_IOTA")"
s_drop="$(measure_n "$BIN_DROP")"
s_append="$(measure_n "$BIN_APPEND")"
s_concat="$(measure_n "$BIN_CONCAT")"
s_rev="$(measure_n "$BIN_REV")"
s_map="$(measure_n "$BIN_MAP")"
s_map_set="$(measure_n "$BIN_MAP_SET")"
s_sort="$(measure_n "$BIN_SORT")"
bench_print_stats "take_heap" "$s_take"
bench_print_stats "take_iota" "$s_take_iota"
bench_print_stats "drop_cons" "$s_drop"
bench_print_stats "take_app" "$s_append"
bench_print_stats "concat_u" "$s_concat"
bench_print_stats "rev_cons" "$s_rev"
bench_print_stats "map_lookup" "$s_map"
bench_print_stats "map_set_u" "$s_map_set"
bench_print_stats "sort_cons" "$s_sort"

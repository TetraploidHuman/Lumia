#!/usr/bin/env bash
# Compiler / frontend pipeline latency (Release host compiler).
# Measures wall time + peak RSS of `lumia check` and `lumia build --release`
# on production-shaped sources (not generated-code microkernels).
#
# Workloads:
#   check_guide_batch  — typecheck a representative guide set
#   check_word_count   — DESIGN §14 flagship
#   build_app          — Release build of bench_app.lm
#   build_task_load    — Release build of bench_task_load.lm
#   build_fuse         — Release build of fuse_hof.lm (HOF pipeline)
#
# Env:
#   RUNS=5               # samples per command (default 5)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
# shellcheck disable=SC1091
source "$ROOT/scripts/bench_measure.sh"

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_bench_compile"
mkdir -p "$OUT_DIR"
RUNS="${RUNS:-5}"

BATCH_SCRIPT="$OUT_DIR/check_batch.sh"
cat > "$BATCH_SCRIPT" <<EOF
#!/usr/bin/env bash
set -euo pipefail
LUMIA="$LUMIA"
for f in \\
  examples/guide/word_count.lm \\
  examples/guide/list_hof.lm \\
  examples/guide/fuse_hof.lm \\
  examples/guide/map_ops.lm \\
  examples/guide/set_algebra.lm \\
  examples/guide/string_more.lm \\
  examples/guide/par_map.lm \\
  examples/guide/memo_dense.lm
do
  "\$LUMIA" check "\$f" >/dev/null
done
EOF
chmod +x "$BATCH_SCRIPT"

echo "======== lumia bench_compile (Release host) ========"

measure_cmd() {
  local name=$1
  shift
  local samples="" i
  for ((i = 1; i <= RUNS; i++)); do
    samples+="$(bench_measure "$@")"$'\n'
  done
  local stats
  stats="$(printf '%s' "$samples" | bench_measure_stats)"
  bench_print_stats "$name" "$stats"
}

echo "== check guide batch (8 files, RUNS=$RUNS) =="
measure_cmd "check_batch" "$BATCH_SCRIPT"

echo "== check word_count =="
measure_cmd "check_wc" "$LUMIA" check examples/guide/word_count.lm

echo "== build --release bench_app =="
measure_cmd "build_app" "$LUMIA" build --release examples/bench/bench_app.lm -o "$OUT_DIR/app"

echo "== build --release bench_task_load =="
measure_cmd "build_task" "$LUMIA" build --release examples/bench/bench_task_load.lm -o "$OUT_DIR/task_load"

echo "== build --release fuse_hof =="
measure_cmd "build_fuse" "$LUMIA" build --release examples/guide/fuse_hof.lm -o "$OUT_DIR/fuse"

echo "bench_compile: OK"

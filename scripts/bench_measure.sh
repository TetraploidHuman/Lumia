# shellcheck shell=bash
# Shared wall-time + peak-RSS measurement for microbenches (source from other scripts).
#
# Peak RSS is max resident set size of the child (Linux: KB via wait4/RUSAGE).
# One sample line: "<elapsed_s> <peak_rss_kb>"
#
# Helpers:
#   bench_measure <cmd> [args...]     → prints one sample line
#   bench_measure_stats               → stdin: sample lines → "t_min t_med t_max  rss_min rss_med rss_max"
#   bench_fmt_rss_kb <kb>             → human MiB string
#   bench_print_stats <name> <stats>  → pretty two-line report

# Resolve repo root even when this file is sourced.
_BENCH_MEASURE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
_BENCH_PEAK_RSS_SRC="$_BENCH_MEASURE_DIR/peak_rss.c"
_BENCH_PEAK_RSS_BIN="${LUMIA_PEAK_RSS_BIN:-$(cd "$_BENCH_MEASURE_DIR/.." && pwd)/target/bench_peak_rss}"

bench_ensure_peak_rss() {
  if [[ -x "$_BENCH_PEAK_RSS_BIN" ]] \
    && [[ "$_BENCH_PEAK_RSS_BIN" -nt "$_BENCH_PEAK_RSS_SRC" ]]; then
    return 0
  fi
  mkdir -p "$(dirname "$_BENCH_PEAK_RSS_BIN")"
  # Small parent avoids Python fork COW inflating ru_maxrss to the interpreter's RSS.
  if ! clang -O2 "$_BENCH_PEAK_RSS_SRC" -o "$_BENCH_PEAK_RSS_BIN" 2>/dev/null; then
    echo "bench_measure: failed to build $_BENCH_PEAK_RSS_BIN (need clang)" >&2
    return 1
  fi
}

bench_measure() {
  # Discard command stdout/stderr; print "elapsed_s peak_rss_kb" on stdout.
  bench_ensure_peak_rss || return 1
  "$_BENCH_PEAK_RSS_BIN" "$@"
}

bench_fmt_rss_kb() {
  awk -v kb="$1" 'BEGIN{
    if (kb == "" || kb == "n/a") { print "n/a"; exit }
    if (kb+0 <= 0) { print "n/a"; exit }
    printf "%.1fMiB", kb/1024.0
  }'
}

# stdin: lines of "time_s rss_kb" → stdout: "t_min t_med t_max rss_min rss_med rss_max"
bench_measure_stats() {
  awk '
    NF >= 1 { t[++nt] = $1 + 0 }
    NF >= 2 { r[++nr] = $2 + 0 }
    function sortn(b, m,    i, j, tmp) {
      for (i = 1; i <= m; i++)
        for (j = i + 1; j <= m; j++)
          if (b[j] < b[i]) { tmp = b[i]; b[i] = b[j]; b[j] = tmp }
    }
    function med(b, m) {
      if (m < 1) return "n/a"
      if (m % 2 == 1) return sprintf("%.4f", b[int((m + 1) / 2)])
      return sprintf("%.4f", (b[m / 2] + b[m / 2 + 1]) / 2)
    }
    function medi(b, m) {
      if (m < 1) return "n/a"
      if (m % 2 == 1) return sprintf("%.0f", b[int((m + 1) / 2)])
      return sprintf("%.0f", (b[m / 2] + b[m / 2 + 1]) / 2)
    }
    END {
      if (nt < 1) { print "n/a n/a n/a n/a n/a n/a"; exit }
      sortn(t, nt)
      sortn(r, nr)
      printf "%.4f %s %.4f %s %s %s\n", t[1], med(t, nt), t[nt], \
        (nr ? sprintf("%.0f", r[1]) : "n/a"), medi(r, nr), \
        (nr ? sprintf("%.0f", r[nr]) : "n/a")
    }
  '
}

bench_print_stats() {
  local name=$1
  local stats=$2
  local t_min t_med t_max r_min r_med r_max
  read -r t_min t_med t_max r_min r_med r_max <<<"$stats"
  printf '%-10s time(s)  min/med/max  %s  %s  %s\n' "$name" "$t_min" "$t_med" "$t_max"
  printf '%-10s rss(KB)  min/med/max  %s  %s  %s  (%s / %s / %s)\n' \
    "$name" "$r_min" "$r_med" "$r_max" \
    "$(bench_fmt_rss_kb "$r_min")" "$(bench_fmt_rss_kb "$r_med")" "$(bench_fmt_rss_kb "$r_max")"
}

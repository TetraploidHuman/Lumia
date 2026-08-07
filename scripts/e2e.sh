#!/usr/bin/env bash
# End-to-end: compile examples and run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=env.sh
source "$ROOT/scripts/env.sh"

cd "$ROOT"
cargo build -p lumia -p lumia_rt
LUMIA="$ROOT/target/debug/lumia"

"$LUMIA" check examples/hello.lumia
"$LUMIA" build examples/hello.lumia -o /tmp/lumia_e2e_hello
out="$(/tmp/lumia_e2e_hello)"
[[ "$out" == "42" ]] || { echo "hello failed: $out"; exit 1; }

"$LUMIA" build examples/add.lumia -o /tmp/lumia_e2e_add
out="$(/tmp/lumia_e2e_add)"
[[ "$out" == "42" ]] || { echo "add failed: $out"; exit 1; }

"$LUMIA" build examples/match.lumia -o /tmp/lumia_e2e_match
out="$(/tmp/lumia_e2e_match)"
[[ "$out" == "20" ]] || { echo "match failed: $out"; exit 1; }

"$LUMIA" build examples/list_for.lumia -o /tmp/lumia_e2e_list_for
out="$(/tmp/lumia_e2e_list_for)"
[[ "$out" == "60" ]] || { echo "list_for failed: $out"; exit 1; }

"$LUMIA" build examples/break.lumia -o /tmp/lumia_e2e_break
out="$(/tmp/lumia_e2e_break)"
[[ "$out" == "4" ]] || { echo "break failed: $out"; exit 1; }

"$LUMIA" build examples/list_match.lumia -o /tmp/lumia_e2e_list_match
out="$(/tmp/lumia_e2e_list_match | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 7" ]] || { echo "list_match failed: $out"; exit 1; }

"$LUMIA" build examples/to_map.lumia -o /tmp/lumia_e2e_to_map
out="$(/tmp/lumia_e2e_to_map)"
[[ "$out" == "2" ]] || { echo "to_map failed: $out"; exit 1; }

"$LUMIA" build examples/map_ops.lumia -o /tmp/lumia_e2e_map_ops
out="$(/tmp/lumia_e2e_map_ops | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 20 10 -1 0 3 1 30 2 2 0 1 0 2 10 1 10" ]] || { echo "map_ops failed: $out"; exit 1; }

"$LUMIA" build examples/option_match.lumia -o /tmp/lumia_e2e_option
out="$(/tmp/lumia_e2e_option | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 7" ]] || { echo "option_match failed: $out"; exit 1; }

"$LUMIA" build examples/point.lumia -o /tmp/lumia_e2e_point
out="$(/tmp/lumia_e2e_point | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 4 10 4 3 7 5 8 3" ]] || { echo "point failed: $out"; exit 1; }

"$LUMIA" build examples/use_math.lumia -o /tmp/lumia_e2e_import
out="$(/tmp/lumia_e2e_import | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_math failed: $out"; exit 1; }

"$LUMIA" build examples/use_priv.lumia -o /tmp/lumia_e2e_priv
out="$(/tmp/lumia_e2e_priv | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_priv failed: $out"; exit 1; }

if "$LUMIA" check examples/bad_import_priv.lumia >/dev/null 2>&1; then
  echo "priv import should fail"; exit 1
fi

"$LUMIA" build examples/use_pkg.lumia -o /tmp/lumia_e2e_pkg
out="$(/tmp/lumia_e2e_pkg | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_pkg failed: $out"; exit 1; }

"$LUMIA" build examples/list_hof.lumia -o /tmp/lumia_e2e_hof
out="$(/tmp/lumia_e2e_hof | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 2 3 24" ]] || { echo "list_hof failed: $out"; exit 1; }

"$LUMIA" build examples/list_concat.lumia -o /tmp/lumia_e2e_concat
out="$(/tmp/lumia_e2e_concat | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 1 5 30" ]] || { echo "list_concat failed: $out"; exit 1; }

"$LUMIA" build examples/list_pipe.lumia -o /tmp/lumia_e2e_pipe
out="$(/tmp/lumia_e2e_pipe | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 6 10" ]] || { echo "list_pipe failed: $out"; exit 1; }

"$LUMIA" build examples/list_set.lumia -o /tmp/lumia_e2e_lset
out="$(/tmp/lumia_e2e_lset | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 99 3 2 3" ]] || { echo "list_set failed: $out"; exit 1; }

"$LUMIA" build examples/match_guard.lumia -o /tmp/lumia_e2e_guard
out="$(/tmp/lumia_e2e_guard | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 2 0" ]] || { echo "match_guard failed: $out"; exit 1; }

"$LUMIA" build examples/logic.lumia -o /tmp/lumia_e2e_logic
out="$(/tmp/lumia_e2e_logic | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 10" ]] || { echo "logic failed: $out"; exit 1; }

"$LUMIA" build examples/string_ops.lumia -o /tmp/lumia_e2e_str
out="$(/tmp/lumia_e2e_str | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 hello 2" ]] || { echo "string_ops failed: $out"; exit 1; }

"$LUMIA" build examples/string_eq.lumia -o /tmp/lumia_e2e_streq
out="$(/tmp/lumia_e2e_streq | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 1 1 1.5" ]] || { echo "string_eq failed: $out"; exit 1; }

"$LUMIA" build examples/fib.lumia -o /tmp/lumia_e2e_fib
out="$(/tmp/lumia_e2e_fib | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "55" ]] || { echo "fib failed: $out"; exit 1; }

"$LUMIA" build examples/char.lumia -o /tmp/lumia_e2e_char
out="$(/tmp/lumia_e2e_char | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "A 1 1 Z" ]] || { echo "char failed: [$out]"; exit 1; }

"$LUMIA" build examples/float_ops.lumia -o /tmp/lumia_e2e_float
out="$(/tmp/lumia_e2e_float | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3.75 6 1 -1.5" ]] || { echo "float_ops failed: [$out]"; exit 1; }

"$LUMIA" build examples/closure.lumia -o /tmp/lumia_e2e_closure
out="$(/tmp/lumia_e2e_closure | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 11" ]] || { echo "closure failed: [$out]"; exit 1; }

"$LUMIA" build examples/closure_capture.lumia -o /tmp/lumia_e2e_cap
out="$(/tmp/lumia_e2e_cap | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 101 42" ]] || { echo "closure_capture failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_hof_fn.lumia -o /tmp/lumia_e2e_hoffn
out="$(/tmp/lumia_e2e_hoffn | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "10 30 1 3 6" ]] || { echo "list_hof_fn failed: [$out]"; exit 1; }

"$LUMIA" build examples/string_interp.lumia -o /tmp/lumia_e2e_interp
out="$(/tmp/lumia_e2e_interp | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello Lumia n=42 43 plain dollar=\$n" ]] || { echo "string_interp failed: [$out]"; exit 1; }

"$LUMIA" build examples/range_fold.lumia -o /tmp/lumia_e2e_rf
out="$(/tmp/lumia_e2e_rf | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "499999500000 5050" ]] || { echo "range_fold failed: [$out]"; exit 1; }

"$LUMIA" build examples/set_ops.lumia -o /tmp/lumia_e2e_set
out="$(/tmp/lumia_e2e_set | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 1 0 3 2 0 1 3 1" ]] || { echo "set_ops failed: [$out]"; exit 1; }

"$LUMIA" build examples/mapset.lumia -o /tmp/lumia_e2e_mapset
out="$(/tmp/lumia_e2e_mapset | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 0 2 3 1 0 4" ]] || { echo "mapset failed: [$out]"; exit 1; }

"$LUMIA" build examples/coll_conv.lumia -o /tmp/lumia_e2e_cc
out="$(/tmp/lumia_e2e_cc | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 1 0 3 2 1" ]] || { echo "coll_conv failed: [$out]"; exit 1; }

"$LUMIA" build examples/set_algebra.lumia -o /tmp/lumia_e2e_sa
out="$(/tmp/lumia_e2e_sa | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "4 1 1 2 1 0 1 1 0" ]] || { echo "set_algebra failed: [$out]"; exit 1; }

"$LUMIA" build examples/for_map_set.lumia -o /tmp/lumia_e2e_fms
out="$(/tmp/lumia_e2e_fms | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "6 3 30" ]] || { echo "for_map_set failed: [$out]"; exit 1; }

"$LUMIA" build examples/range_map.lumia -o /tmp/lumia_e2e_rm
out="$(/tmp/lumia_e2e_rm | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 2 10 5 1 9 249999500000" ]] || { echo "range_map failed: [$out]"; exit 1; }

"$LUMIA" build examples/fuse_hof.lumia -o /tmp/lumia_e2e_fuse
out="$(/tmp/lumia_e2e_fuse | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "24 250500" ]] || { echo "fuse_hof failed: [$out]"; exit 1; }

"$LUMIA" build examples/result_match.lumia -o /tmp/lumia_e2e_res
out="$(/tmp/lumia_e2e_res | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 -1 3" ]] || { echo "result_match failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_extras.lumia -o /tmp/lumia_e2e_lex
out="$(/tmp/lumia_e2e_lex | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 1 4 4 4 1 20 1 0 1 0 2 -1" ]] || { echo "list_extras failed: [$out]"; exit 1; }

"$LUMIA" build examples/prelude_option.lumia -o /tmp/lumia_e2e_po
out="$(/tmp/lumia_e2e_po | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "10 -1 42 7" ]] || { echo "prelude_option failed: [$out]"; exit 1; }

"$LUMIA" build examples/string_more.lumia -o /tmp/lumia_e2e_sm
out="$(/tmp/lumia_e2e_sm | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "11 Hello Lumia 2 Hello Lumia hello lumia HELLO LUMIA Hello 3 3 3 3 3 bar" ]] || { echo "string_more failed: [$out]"; exit 1; }

"$LUMIA" build examples/map_string_keys.lumia -o /tmp/lumia_e2e_msk
out="$(/tmp/lumia_e2e_msk | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2 1 0 2 1 1 1 0" ]] || { echo "map_string_keys failed: [$out]"; exit 1; }

"$LUMIA" build examples/read_stdin.lumia -o /tmp/lumia_e2e_rs
out="$(printf '  hi hi there  ' | /tmp/lumia_e2e_rs | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 hi 2 1 1" ]] || { echo "read_stdin failed: [$out]"; exit 1; }

"$LUMIA" build examples/word_count.lumia -o /tmp/lumia_e2e_wc
out="$(printf 'Hello World\nhello there\nWORLD\n' | /tmp/lumia_e2e_wc | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello: 2 there: 1 world: 2" ]] || { echo "word_count failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_text.lumia -o /tmp/lumia_e2e_lt
out="$(/tmp/lumia_e2e_lt | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2 3 1 2 3 a-b-c 3 3 x z 1 0 2 2" ]] || { echo "list_text failed: [$out]"; exit 1; }

"$LUMIA" build --release examples/memo_l2.lumia -o /tmp/lumia_e2e_memo
out="$(/tmp/lumia_e2e_memo | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2646700 2646700 285" ]] || { echo "memo_l2 failed: [$out]"; exit 1; }

"$LUMIA" build examples/memo_l0l1.lumia -o /tmp/lumia_e2e_m01
out="$(/tmp/lumia_e2e_m01 | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42 65" ]] || { echo "memo_l0l1 failed: [$out]"; exit 1; }

"$LUMIA" build examples/correctness_fixes.lumia -o /tmp/lumia_e2e_cf
out="$(/tmp/lumia_e2e_cf | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 1 1 1 0 0 2 1.25" ]] || { echo "correctness_fixes failed: [$out]"; exit 1; }

echo "e2e ok"

#!/usr/bin/env bash
# End-to-end: compile examples and run.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=env.sh
source "$ROOT/scripts/env.sh"

cd "$ROOT"
cargo build -p lumia -p lumia_rt
LUMIA="$ROOT/target/debug/lumia"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/lumia_e2e.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT

"$LUMIA" check examples/hello.lm
"$LUMIA" build examples/hello.lm -o "$WORKDIR/hello"
out="$("$WORKDIR/hello")"
[[ "$out" == "42" ]] || { echo "hello failed: $out"; exit 1; }

"$LUMIA" build examples/add.lm -o "$WORKDIR/add"
out="$("$WORKDIR/add")"
[[ "$out" == "42" ]] || { echo "add failed: $out"; exit 1; }

"$LUMIA" build examples/match.lm -o "$WORKDIR/match"
out="$("$WORKDIR/match")"
[[ "$out" == "20" ]] || { echo "match failed: $out"; exit 1; }

"$LUMIA" build examples/list_for.lm -o "$WORKDIR/list_for"
out="$("$WORKDIR/list_for")"
[[ "$out" == "60" ]] || { echo "list_for failed: $out"; exit 1; }

"$LUMIA" build examples/break.lm -o "$WORKDIR/break"
out="$("$WORKDIR/break")"
[[ "$out" == "4" ]] || { echo "break failed: $out"; exit 1; }

"$LUMIA" build examples/list_match.lm -o "$WORKDIR/list_match"
out="$("$WORKDIR/list_match" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 7" ]] || { echo "list_match failed: $out"; exit 1; }

"$LUMIA" build examples/to_map.lm -o "$WORKDIR/to_map"
out="$("$WORKDIR/to_map")"
[[ "$out" == "2" ]] || { echo "to_map failed: $out"; exit 1; }

"$LUMIA" build examples/map_ops.lm -o "$WORKDIR/map_ops"
out="$("$WORKDIR/map_ops" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 20 10 -1 0 3 1 30 2 2 0 1 0 2 10 1 10" ]] || { echo "map_ops failed: $out"; exit 1; }

"$LUMIA" build examples/option_match.lm -o "$WORKDIR/option"
out="$("$WORKDIR/option" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 7" ]] || { echo "option_match failed: $out"; exit 1; }

"$LUMIA" build examples/point.lm -o "$WORKDIR/point"
out="$("$WORKDIR/point" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 4 10 4 3 7 5 8 3" ]] || { echo "point failed: $out"; exit 1; }

"$LUMIA" build examples/use_math.lm -o "$WORKDIR/import"
out="$("$WORKDIR/import" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_math failed: $out"; exit 1; }

"$LUMIA" build examples/use_priv.lm -o "$WORKDIR/priv"
out="$("$WORKDIR/priv" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_priv failed: $out"; exit 1; }

if "$LUMIA" check examples/bad_import_priv.lm >/dev/null 2>&1; then
  echo "priv import should fail"; exit 1
fi

"$LUMIA" build examples/use_pkg.lm -o "$WORKDIR/pkg"
out="$("$WORKDIR/pkg" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_pkg failed: $out"; exit 1; }

"$LUMIA" build examples/list_hof.lm -o "$WORKDIR/hof"
out="$("$WORKDIR/hof" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 2 3 24" ]] || { echo "list_hof failed: $out"; exit 1; }

"$LUMIA" build examples/list_concat.lm -o "$WORKDIR/concat"
out="$("$WORKDIR/concat" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 1 5 30" ]] || { echo "list_concat failed: $out"; exit 1; }

"$LUMIA" build examples/list_pipe.lm -o "$WORKDIR/pipe"
out="$("$WORKDIR/pipe" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 6 10" ]] || { echo "list_pipe failed: $out"; exit 1; }

"$LUMIA" build examples/list_set.lm -o "$WORKDIR/lset"
out="$("$WORKDIR/lset" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 99 3 2 3" ]] || { echo "list_set failed: $out"; exit 1; }

"$LUMIA" build examples/match_guard.lm -o "$WORKDIR/guard"
out="$("$WORKDIR/guard" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 2 0" ]] || { echo "match_guard failed: $out"; exit 1; }

"$LUMIA" build examples/logic.lm -o "$WORKDIR/logic"
out="$("$WORKDIR/logic" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 10" ]] || { echo "logic failed: $out"; exit 1; }

"$LUMIA" build examples/string_ops.lm -o "$WORKDIR/str"
out="$("$WORKDIR/str" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 hello 2" ]] || { echo "string_ops failed: $out"; exit 1; }

"$LUMIA" build examples/string_eq.lm -o "$WORKDIR/streq"
out="$("$WORKDIR/streq" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 1 1 1.5" ]] || { echo "string_eq failed: $out"; exit 1; }

"$LUMIA" build examples/fib.lm -o "$WORKDIR/fib"
out="$("$WORKDIR/fib" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "55" ]] || { echo "fib failed: $out"; exit 1; }

"$LUMIA" build examples/char.lm -o "$WORKDIR/char"
out="$("$WORKDIR/char" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "A 1 1 Z" ]] || { echo "char failed: [$out]"; exit 1; }

"$LUMIA" build examples/float_ops.lm -o "$WORKDIR/float"
out="$("$WORKDIR/float" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3.75 6 1 -1.5" ]] || { echo "float_ops failed: [$out]"; exit 1; }

"$LUMIA" build examples/closure.lm -o "$WORKDIR/closure"
out="$("$WORKDIR/closure" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 11" ]] || { echo "closure failed: [$out]"; exit 1; }

"$LUMIA" build examples/closure_capture.lm -o "$WORKDIR/cap"
out="$("$WORKDIR/cap" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 101 42" ]] || { echo "closure_capture failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_hof_fn.lm -o "$WORKDIR/hoffn"
out="$("$WORKDIR/hoffn" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "10 30 1 3 6" ]] || { echo "list_hof_fn failed: [$out]"; exit 1; }

"$LUMIA" build examples/string_interp.lm -o "$WORKDIR/interp"
out="$("$WORKDIR/interp" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello Lumia n=42 43 plain dollar=\$n" ]] || { echo "string_interp failed: [$out]"; exit 1; }

"$LUMIA" build examples/range_fold.lm -o "$WORKDIR/rf"
out="$("$WORKDIR/rf" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "499999500000 5050" ]] || { echo "range_fold failed: [$out]"; exit 1; }

"$LUMIA" build examples/set_ops.lm -o "$WORKDIR/set"
out="$("$WORKDIR/set" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 1 0 3 2 0 1 3 1" ]] || { echo "set_ops failed: [$out]"; exit 1; }

"$LUMIA" build examples/mapset.lm -o "$WORKDIR/mapset"
out="$("$WORKDIR/mapset" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 0 2 3 1 0 4" ]] || { echo "mapset failed: [$out]"; exit 1; }

"$LUMIA" build examples/coll_conv.lm -o "$WORKDIR/cc"
out="$("$WORKDIR/cc" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 1 0 3 2 1" ]] || { echo "coll_conv failed: [$out]"; exit 1; }

"$LUMIA" build examples/set_algebra.lm -o "$WORKDIR/sa"
out="$("$WORKDIR/sa" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "4 1 1 2 1 0 1 1 0" ]] || { echo "set_algebra failed: [$out]"; exit 1; }

"$LUMIA" build examples/for_map_set.lm -o "$WORKDIR/fms"
out="$("$WORKDIR/fms" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "6 3 30" ]] || { echo "for_map_set failed: [$out]"; exit 1; }

"$LUMIA" build examples/range_map.lm -o "$WORKDIR/rm"
out="$("$WORKDIR/rm" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 2 10 5 1 9 249999500000" ]] || { echo "range_map failed: [$out]"; exit 1; }

"$LUMIA" build examples/fuse_hof.lm -o "$WORKDIR/fuse"
out="$("$WORKDIR/fuse" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "24 250500" ]] || { echo "fuse_hof failed: [$out]"; exit 1; }

"$LUMIA" build examples/result_match.lm -o "$WORKDIR/res"
out="$("$WORKDIR/res" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 -1 3" ]] || { echo "result_match failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_extras.lm -o "$WORKDIR/lex"
out="$("$WORKDIR/lex" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 1 4 4 4 1 20 1 0 1 0 2 -1" ]] || { echo "list_extras failed: [$out]"; exit 1; }

"$LUMIA" build examples/prelude_option.lm -o "$WORKDIR/po"
out="$("$WORKDIR/po" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "10 -1 42 7" ]] || { echo "prelude_option failed: [$out]"; exit 1; }

"$LUMIA" build examples/string_more.lm -o "$WORKDIR/sm"
out="$("$WORKDIR/sm" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "11 Hello Lumia 2 Hello Lumia hello lumia HELLO LUMIA Hello 3 3 3 3 3 bar" ]] || { echo "string_more failed: [$out]"; exit 1; }

"$LUMIA" build examples/map_string_keys.lm -o "$WORKDIR/msk"
out="$("$WORKDIR/msk" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2 1 0 2 1 1 1 0" ]] || { echo "map_string_keys failed: [$out]"; exit 1; }

"$LUMIA" build examples/read_stdin.lm -o "$WORKDIR/rs"
out="$(printf '  hi hi there  ' | "$WORKDIR/rs" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 hi 2 1 1" ]] || { echo "read_stdin failed: [$out]"; exit 1; }

"$LUMIA" build examples/word_count.lm -o "$WORKDIR/wc"
out="$(printf 'Hello World\nhello there\nWORLD\n' | "$WORKDIR/wc" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello: 2 there: 1 world: 2" ]] || { echo "word_count failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_text.lm -o "$WORKDIR/lt"
out="$("$WORKDIR/lt" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2 3 1 2 3 a-b-c 3 3 x z 1 0 2 2" ]] || { echo "list_text failed: [$out]"; exit 1; }

"$LUMIA" build --release examples/memo_l2.lm -o "$WORKDIR/memo"
out="$("$WORKDIR/memo" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2646700 2646700 285" ]] || { echo "memo_l2 failed: [$out]"; exit 1; }

"$LUMIA" build examples/memo_l0l1.lm -o "$WORKDIR/m01"
out="$("$WORKDIR/m01" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42 65" ]] || { echo "memo_l0l1 failed: [$out]"; exit 1; }

"$LUMIA" build examples/correctness_fixes.lm -o "$WORKDIR/cf"
out="$("$WORKDIR/cf" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 1 1 1 0 0 2 1.25 2 2" ]] || { echo "correctness_fixes failed: [$out]"; exit 1; }

"$LUMIA" build examples/scope_shadow.lm -o "$WORKDIR/scope"
out="$("$WORKDIR/scope" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "99 1 1 99 1" ]] || { echo "scope_shadow failed: [$out]"; exit 1; }

"$LUMIA" build examples/result_branch.lm -o "$WORKDIR/rbranch"
out="$("$WORKDIR/rbranch" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "7 -1" ]] || { echo "result_branch failed: [$out]"; exit 1; }

"$LUMIA" build examples/module_val_str.lm -o "$WORKDIR/mvs"
out="$("$WORKDIR/mvs" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello 4" ]] || { echo "module_val_str failed: [$out]"; exit 1; }

"$LUMIA" build examples/for_pair_list.lm -o "$WORKDIR/fpl"
out="$("$WORKDIR/fpl" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "66" ]] || { echo "for_pair_list failed: [$out]"; exit 1; }

"$LUMIA" build examples/hof_float_to_int.lm -o "$WORKDIR/hfi"
out="$("$WORKDIR/hfi" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 2" ]] || { echo "hof_float_to_int failed: [$out]"; exit 1; }

echo "e2e ok"

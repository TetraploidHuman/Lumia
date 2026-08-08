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

"$LUMIA" check examples/hello.lumia
"$LUMIA" build examples/hello.lumia -o "$WORKDIR/hello"
out="$("$WORKDIR/hello")"
[[ "$out" == "42" ]] || { echo "hello failed: $out"; exit 1; }

"$LUMIA" build examples/add.lumia -o "$WORKDIR/add"
out="$("$WORKDIR/add")"
[[ "$out" == "42" ]] || { echo "add failed: $out"; exit 1; }

"$LUMIA" build examples/match.lumia -o "$WORKDIR/match"
out="$("$WORKDIR/match")"
[[ "$out" == "20" ]] || { echo "match failed: $out"; exit 1; }

"$LUMIA" build examples/list_for.lumia -o "$WORKDIR/list_for"
out="$("$WORKDIR/list_for")"
[[ "$out" == "60" ]] || { echo "list_for failed: $out"; exit 1; }

"$LUMIA" build examples/break.lumia -o "$WORKDIR/break"
out="$("$WORKDIR/break")"
[[ "$out" == "4" ]] || { echo "break failed: $out"; exit 1; }

"$LUMIA" build examples/list_match.lumia -o "$WORKDIR/list_match"
out="$("$WORKDIR/list_match" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 7" ]] || { echo "list_match failed: $out"; exit 1; }

"$LUMIA" build examples/to_map.lumia -o "$WORKDIR/to_map"
out="$("$WORKDIR/to_map")"
[[ "$out" == "2" ]] || { echo "to_map failed: $out"; exit 1; }

"$LUMIA" build examples/map_ops.lumia -o "$WORKDIR/map_ops"
out="$("$WORKDIR/map_ops" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 20 10 -1 0 3 1 30 2 2 0 1 0 2 10 1 10" ]] || { echo "map_ops failed: $out"; exit 1; }

"$LUMIA" build examples/option_match.lumia -o "$WORKDIR/option"
out="$("$WORKDIR/option" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 7" ]] || { echo "option_match failed: $out"; exit 1; }

"$LUMIA" build examples/point.lumia -o "$WORKDIR/point"
out="$("$WORKDIR/point" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 4 10 4 3 7 5 8 3" ]] || { echo "point failed: $out"; exit 1; }

"$LUMIA" build examples/use_math.lumia -o "$WORKDIR/import"
out="$("$WORKDIR/import" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_math failed: $out"; exit 1; }

"$LUMIA" build examples/use_priv.lumia -o "$WORKDIR/priv"
out="$("$WORKDIR/priv" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_priv failed: $out"; exit 1; }

if "$LUMIA" check examples/bad_import_priv.lumia >/dev/null 2>&1; then
  echo "priv import should fail"; exit 1
fi

"$LUMIA" build examples/use_pkg.lumia -o "$WORKDIR/pkg"
out="$("$WORKDIR/pkg" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42" ]] || { echo "use_pkg failed: $out"; exit 1; }

"$LUMIA" build examples/list_hof.lumia -o "$WORKDIR/hof"
out="$("$WORKDIR/hof" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 2 3 24" ]] || { echo "list_hof failed: $out"; exit 1; }

"$LUMIA" build examples/list_concat.lumia -o "$WORKDIR/concat"
out="$("$WORKDIR/concat" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 1 5 30" ]] || { echo "list_concat failed: $out"; exit 1; }

"$LUMIA" build examples/list_pipe.lumia -o "$WORKDIR/pipe"
out="$("$WORKDIR/pipe" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 6 10" ]] || { echo "list_pipe failed: $out"; exit 1; }

"$LUMIA" build examples/list_set.lumia -o "$WORKDIR/lset"
out="$("$WORKDIR/lset" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 99 3 2 3" ]] || { echo "list_set failed: $out"; exit 1; }

"$LUMIA" build examples/match_guard.lumia -o "$WORKDIR/guard"
out="$("$WORKDIR/guard" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 2 0" ]] || { echo "match_guard failed: $out"; exit 1; }

"$LUMIA" build examples/logic.lumia -o "$WORKDIR/logic"
out="$("$WORKDIR/logic" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 10" ]] || { echo "logic failed: $out"; exit 1; }

"$LUMIA" build examples/string_ops.lumia -o "$WORKDIR/str"
out="$("$WORKDIR/str" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 hello 2" ]] || { echo "string_ops failed: $out"; exit 1; }

"$LUMIA" build examples/string_eq.lumia -o "$WORKDIR/streq"
out="$("$WORKDIR/streq" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 1 1 1.5" ]] || { echo "string_eq failed: $out"; exit 1; }

"$LUMIA" build examples/fib.lumia -o "$WORKDIR/fib"
out="$("$WORKDIR/fib" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "55" ]] || { echo "fib failed: $out"; exit 1; }

"$LUMIA" build examples/char.lumia -o "$WORKDIR/char"
out="$("$WORKDIR/char" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "A 1 1 Z" ]] || { echo "char failed: [$out]"; exit 1; }

"$LUMIA" build examples/float_ops.lumia -o "$WORKDIR/float"
out="$("$WORKDIR/float" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3.75 6 1 -1.5" ]] || { echo "float_ops failed: [$out]"; exit 1; }

"$LUMIA" build examples/closure.lumia -o "$WORKDIR/closure"
out="$("$WORKDIR/closure" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 11" ]] || { echo "closure failed: [$out]"; exit 1; }

"$LUMIA" build examples/closure_capture.lumia -o "$WORKDIR/cap"
out="$("$WORKDIR/cap" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 101 42" ]] || { echo "closure_capture failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_hof_fn.lumia -o "$WORKDIR/hoffn"
out="$("$WORKDIR/hoffn" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "10 30 1 3 6" ]] || { echo "list_hof_fn failed: [$out]"; exit 1; }

"$LUMIA" build examples/string_interp.lumia -o "$WORKDIR/interp"
out="$("$WORKDIR/interp" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello Lumia n=42 43 plain dollar=\$n" ]] || { echo "string_interp failed: [$out]"; exit 1; }

"$LUMIA" build examples/range_fold.lumia -o "$WORKDIR/rf"
out="$("$WORKDIR/rf" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "499999500000 5050" ]] || { echo "range_fold failed: [$out]"; exit 1; }

"$LUMIA" build examples/set_ops.lumia -o "$WORKDIR/set"
out="$("$WORKDIR/set" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 1 0 3 2 0 1 3 1" ]] || { echo "set_ops failed: [$out]"; exit 1; }

"$LUMIA" build examples/mapset.lumia -o "$WORKDIR/mapset"
out="$("$WORKDIR/mapset" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 0 2 3 1 0 4" ]] || { echo "mapset failed: [$out]"; exit 1; }

"$LUMIA" build examples/coll_conv.lumia -o "$WORKDIR/cc"
out="$("$WORKDIR/cc" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 1 0 3 2 1" ]] || { echo "coll_conv failed: [$out]"; exit 1; }

"$LUMIA" build examples/set_algebra.lumia -o "$WORKDIR/sa"
out="$("$WORKDIR/sa" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "4 1 1 2 1 0 1 1 0" ]] || { echo "set_algebra failed: [$out]"; exit 1; }

"$LUMIA" build examples/for_map_set.lumia -o "$WORKDIR/fms"
out="$("$WORKDIR/fms" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "6 3 30" ]] || { echo "for_map_set failed: [$out]"; exit 1; }

"$LUMIA" build examples/range_map.lumia -o "$WORKDIR/rm"
out="$("$WORKDIR/rm" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 2 10 5 1 9 249999500000" ]] || { echo "range_map failed: [$out]"; exit 1; }

"$LUMIA" build examples/fuse_hof.lumia -o "$WORKDIR/fuse"
out="$("$WORKDIR/fuse" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "24 250500" ]] || { echo "fuse_hof failed: [$out]"; exit 1; }

"$LUMIA" build examples/result_match.lumia -o "$WORKDIR/res"
out="$("$WORKDIR/res" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "5 -1 3" ]] || { echo "result_match failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_extras.lumia -o "$WORKDIR/lex"
out="$("$WORKDIR/lex" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 1 4 4 4 1 20 1 0 1 0 2 -1" ]] || { echo "list_extras failed: [$out]"; exit 1; }

"$LUMIA" build examples/prelude_option.lumia -o "$WORKDIR/po"
out="$("$WORKDIR/po" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "10 -1 42 7" ]] || { echo "prelude_option failed: [$out]"; exit 1; }

"$LUMIA" build examples/string_more.lumia -o "$WORKDIR/sm"
out="$("$WORKDIR/sm" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "11 Hello Lumia 2 Hello Lumia hello lumia HELLO LUMIA Hello 3 3 3 3 3 bar" ]] || { echo "string_more failed: [$out]"; exit 1; }

"$LUMIA" build examples/map_string_keys.lumia -o "$WORKDIR/msk"
out="$("$WORKDIR/msk" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2 1 0 2 1 1 1 0" ]] || { echo "map_string_keys failed: [$out]"; exit 1; }

"$LUMIA" build examples/read_stdin.lumia -o "$WORKDIR/rs"
out="$(printf '  hi hi there  ' | "$WORKDIR/rs" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "3 hi 2 1 1" ]] || { echo "read_stdin failed: [$out]"; exit 1; }

"$LUMIA" build examples/word_count.lumia -o "$WORKDIR/wc"
out="$(printf 'Hello World\nhello there\nWORLD\n' | "$WORKDIR/wc" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello: 2 there: 1 world: 2" ]] || { echo "word_count failed: [$out]"; exit 1; }

"$LUMIA" build examples/list_text.lumia -o "$WORKDIR/lt"
out="$("$WORKDIR/lt" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2 3 1 2 3 a-b-c 3 3 x z 1 0 2 2" ]] || { echo "list_text failed: [$out]"; exit 1; }

"$LUMIA" build --release examples/memo_l2.lumia -o "$WORKDIR/memo"
out="$("$WORKDIR/memo" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "2646700 2646700 285" ]] || { echo "memo_l2 failed: [$out]"; exit 1; }

"$LUMIA" build examples/memo_l0l1.lumia -o "$WORKDIR/m01"
out="$("$WORKDIR/m01" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "42 42 65" ]] || { echo "memo_l0l1 failed: [$out]"; exit 1; }

"$LUMIA" build examples/correctness_fixes.lumia -o "$WORKDIR/cf"
out="$("$WORKDIR/cf" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "0 1 1 1 0 0 2 1.25 2 2" ]] || { echo "correctness_fixes failed: [$out]"; exit 1; }

"$LUMIA" build examples/scope_shadow.lumia -o "$WORKDIR/scope"
out="$("$WORKDIR/scope" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "99 1 1 99 1" ]] || { echo "scope_shadow failed: [$out]"; exit 1; }

"$LUMIA" build examples/result_branch.lumia -o "$WORKDIR/rbranch"
out="$("$WORKDIR/rbranch" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "7 -1" ]] || { echo "result_branch failed: [$out]"; exit 1; }

"$LUMIA" build examples/module_val_str.lumia -o "$WORKDIR/mvs"
out="$("$WORKDIR/mvs" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "hello 4" ]] || { echo "module_val_str failed: [$out]"; exit 1; }

"$LUMIA" build examples/for_pair_list.lumia -o "$WORKDIR/fpl"
out="$("$WORKDIR/fpl" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "66" ]] || { echo "for_pair_list failed: [$out]"; exit 1; }

"$LUMIA" build examples/hof_float_to_int.lumia -o "$WORKDIR/hfi"
out="$("$WORKDIR/hfi" | tr '\n' ' ' | sed 's/ $//')"
[[ "$out" == "1 2" ]] || { echo "hof_float_to_int failed: [$out]"; exit 1; }

echo "e2e ok"

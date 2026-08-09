e2e_ok!(e2e_hello, "examples/hello.lm", "42");

e2e_ok!(
    e2e_bool_print,
    "examples/bool_print.lm",
    "true",
    "false",
    "true",
    "false",
    "true",
    "false",
    "flag=true",
    "flag=false"
);

e2e_ok!(e2e_for, "examples/for.lm", "15", "3");

e2e_ok!(e2e_list, "examples/list.lm", "42");

// `examples/math.lm` / `math_priv.lm` are library modules (no `main`); covered by
// `use_math` / `use_priv`. `bench_*.lm` are timing harnesses, not e2e.

e2e_ok!(e2e_alt_option, "examples/alt_option.lm", "10", "42");

e2e_ok!(
    e2e_alt_option_swap_tags,
    "examples/alt_option_swap_tags.lm",
    "7",
    "9"
);

e2e_ok!(
    e2e_alt_result_swap_tags,
    "examples/alt_result_swap_tags.lm",
    "7",
    "9"
);

e2e_ok!(
    e2e_alt_result_return,
    "examples/alt_result_return.lm",
    "6",
    "-1"
);

e2e_ok!(
    e2e_alt_option_return,
    "examples/alt_option_return.lm",
    "10",
    "-1"
);

e2e_ok!(e2e_zero_arg_return, "examples/zero_arg_return.lm", "42");

e2e_ok!(e2e_return_capture, "examples/return_capture.lm", "42", "7");

e2e_ok!(e2e_return_dead, "examples/return_dead.lm", "3", "0", "42");

e2e_ok!(e2e_add, "examples/add.lm", "42");

e2e_ok!(e2e_match, "examples/match.lm", "20");

e2e_ok!(
    e2e_const_patterns,
    "examples/const_patterns.lm",
    "1",
    "2",
    "3",
    "4",
    "5"
);

e2e_ok!(e2e_pure_io_thunk, "examples/pure_io_thunk.lm", "7");

#[test]
fn e2e_map_adt_assoc() {
    // No `instance Hash` → assoc list; still correct after growing past SmallMap.
    run_example(
        "examples/map_adt_assoc.lm",
        &["20", "0", "38", "true", "false"],
    );
}

#[test]
fn e2e_map_adt_hash() {
    run_example(
        "examples/map_adt_hash.lm",
        &["20", "0", "38", "true", "false"],
    );
}

#[test]
fn e2e_std_option() {
    // Source-backed `std.option` combinators (inlined from std/option.lm).
    run_example(
        "examples/std_option.lm",
        &["21", "-1", "3", "5", "-1", "true", "true"],
    );
}

#[test]
fn e2e_std_result() {
    run_example(
        "examples/std_result.lm",
        &["42", "-1", "3", "5", "odd", "boom!", "true", "true"],
    );
}

e2e_ok!(e2e_list_for, "examples/list_for.lm", "60");

e2e_ok!(e2e_break, "examples/break.lm", "4");

e2e_ok!(e2e_list_match, "examples/list_match.lm", "0", "7");

e2e_ok!(e2e_to_map, "examples/to_map.lm", "2");

e2e_ok!(e2e_option_match, "examples/option_match.lm", "0", "7");

#[test]
fn e2e_point() {
    run_example(
        "examples/point.lm",
        &["3", "4", "10", "4", "3", "7", "5", "8", "3"],
    );
}

e2e_ok!(e2e_use_math, "examples/use_math.lm", "42", "42");

#[test]
fn e2e_doc_std_io() {
    let root = workspace_root();
    let src = root.join("std/io.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["doc", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia doc");
    assert!(out.status.success(), "lumia doc failed: {:?}", out);
    let md = String::from_utf8_lossy(&out.stdout);
    assert!(
        md.contains("# Module `io`")
            && md.contains("Standard I/O")
            && md.contains("**Exports:** `println`, `readStdin`, `assert`"),
        "unexpected doc output: {md}"
    );
}

e2e_ok!(e2e_import_as, "examples/import_as.lm", "42", "42");

e2e_ok!(e2e_use_priv, "examples/use_priv.lm", "42", "42");

e2e_ok!(e2e_use_pkg, "examples/use_pkg.lm", "42", "42");

e2e_ok!(e2e_list_hof, "examples/list_hof.lm", "5", "2", "3", "24");

e2e_ok!(
    e2e_list_hof_fn,
    "examples/list_hof_fn.lm",
    "10",
    "30",
    "1",
    "3",
    "6"
);

e2e_ok!(
    e2e_list_concat,
    "examples/list_concat.lm",
    "5",
    "1",
    "5",
    "30"
);

e2e_ok!(e2e_list_pipe, "examples/list_pipe.lm", "3", "6", "10");

e2e_ok!(
    e2e_list_set,
    "examples/list_set.lm",
    "1",
    "99",
    "3",
    "2",
    "3"
);

e2e_ok!(e2e_match_guard, "examples/match_guard.lm", "1", "2", "0");

e2e_ok!(e2e_match_cond, "examples/match_cond.lm", "1", "0", "-1");

e2e_ok!(e2e_logic, "examples/logic.lm", "1", "10");

e2e_ok!(e2e_string_ops, "examples/string_ops.lm", "5", "hello", "2");

e2e_ok!(e2e_string_eq, "examples/string_eq.lm", "1", "1", "1", "1.5");

#[test]
fn e2e_string_interp() {
    run_example(
        "examples/string_interp.lm",
        &["hello Lumia", "n=42", "43", "plain", "dollar=$n"],
    );
}

e2e_ok!(e2e_fib, "examples/fib.lm", "55");

e2e_ok!(e2e_char, "examples/char.lm", "A", "1", "1", "Z");

e2e_ok!(e2e_closure, "examples/closure.lm", "42", "11");

e2e_ok!(
    e2e_closure_capture,
    "examples/closure_capture.lm",
    "42",
    "101",
    "42"
);

#[test]
fn e2e_map_ops() {
    run_example(
        "examples/map_ops.lm",
        &[
            "true", "20", "10", "-1", "false", "3", "true", "30", "2", "2", "false", "true",
            "false", "2", "10", "1", "10",
        ],
    );
}

#[test]
fn e2e_set_ops() {
    run_example(
        "examples/set_ops.lm",
        &["3", "true", "false", "3", "2", "false", "true", "3", "true"],
    );
}

e2e_ok!(
    e2e_range_fold,
    "examples/range_fold.lm",
    "499999500000",
    "5050"
);

#[test]
fn e2e_mapset() {
    run_example(
        "examples/mapset.lm",
        &["3", "0", "2", "3", "true", "false", "4"],
    );
}

#[test]
fn e2e_coll_lit() {
    run_example(
        "examples/coll_lit.lm",
        &["0", "3", "true", "20", "0", "3", "true", "false", "3", "1"],
    );
}

#[test]
fn e2e_coll_conv() {
    run_example(
        "examples/coll_conv.lm",
        &["3", "true", "false", "3", "2", "true"],
    );
}

#[test]
fn e2e_set_algebra() {
    run_example(
        "examples/set_algebra.lm",
        &[
            "4", "true", "true", "2", "true", "false", "1", "true", "false",
        ],
    );
}

e2e_ok!(e2e_for_map_set, "examples/for_map_set.lm", "6", "3", "30");

#[test]
fn e2e_range_map() {
    run_example(
        "examples/range_map.lm",
        &["5", "2", "10", "5", "1", "9", "249999500000"],
    );
}

#[test]
fn e2e_range_iota() {
    run_example(
        "examples/range_iota.lm",
        &["1000000", "0", "999999", "2", "10", "3", "3"],
    );
}

e2e_ok!(e2e_fuse_hof, "examples/fuse_hof.lm", "24", "250500");

e2e_ok!(e2e_result_match, "examples/result_match.lm", "5", "-1", "3");

#[test]
fn e2e_list_extras() {
    run_example(
        "examples/list_extras.lm",
        &[
            "false", "true", "4", "4", "4", "1", "20", "true", "false", "true", "false", "2", "-1",
        ],
    );
}

#[test]
fn e2e_prelude_option() {
    run_example("examples/prelude_option.lm", &["10", "-1", "42", "7"]);
}

#[test]
fn e2e_string_more() {
    run_example(
        "examples/string_more.lm",
        &[
            "11",
            "Hello Lumia",
            "2",
            "Hello",
            "Lumia",
            "hello lumia",
            "HELLO LUMIA",
            "Hello",
            "3",
            "3",
            "3",
            "3",
            "3",
            "bar",
        ],
    );
}

#[test]
fn e2e_map_string_keys() {
    run_example(
        "examples/map_string_keys.lm",
        &["2", "true", "false", "2", "1", "true", "true", "false"],
    );
}

#[test]
fn e2e_read_stdin() {
    run_example_with_stdin(
        "examples/read_stdin.lm",
        Some("  hi hi there  "),
        &["3", "hi", "2", "true", "true"],
    );
}

#[test]
fn e2e_word_count() {
    run_example_with_stdin(
        "examples/word_count.lm",
        Some("Hello World\nhello there\nWORLD\n"),
        &["hello: 2", "there: 1", "world: 2"],
    );
}

#[test]
fn e2e_list_text() {
    run_example(
        "examples/list_text.lm",
        &[
            "2", "3", "1", "2", "3", "a-b-c", "3", "3", "x", "z", "true", "false", "2", "2",
        ],
    );
}

// Transparent Memo L2 is enabled under `--release`; results must match.

#[test]
fn e2e_correctness_fixes() {
    run_example(
        "examples/correctness_fixes.lm",
        &["0", "1", "1", "1", "0", "0", "2", "1.25", "2", "2"],
    );
}

e2e_ok!(
    e2e_scope_shadow,
    "examples/scope_shadow.lm",
    "99",
    "1",
    "1",
    "99",
    "1"
);

e2e_ok!(e2e_result_branch, "examples/result_branch.lm", "7", "-1");

e2e_ok!(
    e2e_result_err_payload,
    "examples/result_err_payload.lm",
    "42",
    "4"
);

e2e_ok!(e2e_for_map_keys, "examples/for_map_keys.lm", "3", "2", "3");

#[test]
fn e2e_contains_poly() {
    run_example(
        "examples/contains_poly.lm",
        &["true", "false", "true", "false"],
    );
}

e2e_ok!(
    e2e_module_val_str,
    "examples/module_val_str.lm",
    "hello",
    "4"
);

e2e_ok!(e2e_for_pair_list, "examples/for_pair_list.lm", "66");

#[test]
fn e2e_for_destructure() {
    // Map + List[(K,V)] both support `for (k, v) in …`.
    run_example("examples/for_destructure.lm", &["33", "18"]);
}

#[test]
fn e2e_gc_roots() {
    // Soft-threshold GC must not free `keep` while junk lists allocate.
    run_example("examples/gc_roots.lm", &["1", "3"]);
}

#[test]
fn e2e_map_hash() {
    run_example(
        "examples/map_hash.lm",
        &[
            "40", "0", "117", "-1", "true", "false", "777", "39", "false", "3", "1",
        ],
    );
}

#[test]
fn e2e_set_hash() {
    run_example(
        "examples/set_hash.lm",
        &[
            "40", "true", "true", "false", "40", "true", "39", "false", "true", "1",
        ],
    );
}

#[test]
fn e2e_sort_by() {
    run_example(
        "examples/sort_by.lm",
        &[
            "1", "1", "3", "5", "5", "4", "3", "20", "10", "30", "apple", "banana", "cherry",
        ],
    );
}

#[test]
fn e2e_tuple_fields() {
    run_example(
        "examples/tuple_fields.lm",
        &["10", "20", "30", "200", "100", "300"],
    );
}

#[test]
fn e2e_let_destructure() {
    // `val (a, b) = p` / nested tuple / product struct pattern.
    run_example(
        "examples/let_destructure.lm",
        &["10", "20", "6", "7", "8", "30"],
    );
}

e2e_ok!(e2e_effect_hof, "examples/effect_hof.lm", "41", "42");

e2e_ok!(e2e_effect_block, "examples/effect_block.lm", "42");

#[test]
fn e2e_nested_match() {
    run_example(
        "examples/nested_match.lm",
        &["7", "99", "1", "2", "1", "42", "1"],
    );
}

e2e_ok!(e2e_assert_ok, "examples/assert_ok.lm", "1");

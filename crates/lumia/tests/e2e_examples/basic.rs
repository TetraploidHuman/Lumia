e2e_ok!(e2e_hello, "examples/guide/hello.lm", "42");

e2e_ok!(e2e_unit_print, "examples/guide/unit_print.lm", "1", "Unit");

e2e_ok!(e2e_color_show, "examples/guide/color_show.lm", "A", "B", "C");

e2e_ok!(
    e2e_nested_show,
    "examples/guide/nested_show.lm",
    "Some(A)",
    "[A, B]",
    "[Box(1), Box(2)]",
    "Some(Box(9))"
);

e2e_ok!(
    e2e_sum_show_masks,
    "examples/guide/sum_show_masks.lm",
    "[Circle(1.5), Rect(2, 5)]",
    "[Left(1.5), Right(7)]",
    "[Flag(true), Pair(1, 2)]"
);

e2e_ok!(
    e2e_nested_map_get_show,
    "examples/guide/nested_map_get_show.lm",
    "Some(2.5)",
    "Box(Some(2.5))",
    "[Some(2.5)]",
    "Box(Some(true))",
    "[Some(true)]"
);

e2e_ok!(
    e2e_nested_map_items_show,
    "examples/guide/nested_map_items_show.lm",
    "{true: false}",
    "[#0(true, false)]",
    "Box([#0(true, false)])",
    "[#0(true, false)]",
    "[#0(1.5, 2.5)]",
    "Box([#0(1.5, 2.5)])"
);

e2e_ok!(
    e2e_nested_bool_container_show,
    "examples/guide/nested_bool_container_show.lm",
    "[true, false]",
    "Box([true, false])",
    "[true, true]",
    "Box([true, true])",
    "{true: false}",
    "Box({true: false})",
    "[true]",
    "Box([true])",
    "[false]",
    "Box([false])",
    "#{true, false}",
    "Box(#{true, false})",
    "Box([1.5])"
);

e2e_ok!(
    e2e_bool_print,
    "examples/guide/bool_print.lm",
    "true",
    "false",
    "true",
    "false",
    "true",
    "false",
    "flag=true",
    "flag=false"
);

e2e_ok!(
    e2e_mut_map_set_bool_show,
    "examples/guide/mut_map_set_bool_show.lm",
    "{true: false, false: true}",
    "#{true, false}"
);

e2e_ok!(
    e2e_empty_map_set_eq,
    "examples/guide/empty_map_set_eq.lm",
    "{}",
    "{}",
    "true",
    "#{}",
    "#{}",
    "true"
);

e2e_ok!(
    e2e_nested_empty_map_set_show,
    "examples/guide/nested_empty_map_set_show.lm",
    "{}",
    "Some({})",
    "[{}]",
    "Box({})",
    "Some(#{})",
    "Box(#{})",
    "{}",
    "Some({})"
);

e2e_ok!(e2e_nested_it_map, "examples/guide/nested_it_map.lm", "[2, 3, 4]");

e2e_ok!(
    e2e_par_map_float_to_int,
    "examples/guide/par_map_float_to_int.lm",
    "[1, 1]",
    "1",
    "[1]",
    "[1.5, 2.5]",
    "[2.5, 3.5]"
);

e2e_ok!(e2e_for, "examples/guide/for.lm", "15", "3");

e2e_ok!(e2e_list, "examples/guide/list.lm", "42");

// `examples/guide/math.lm` / `math_priv.lm` are library modules (no `main`); covered by
// `use_math` / `use_priv`. Full `bench_cpu.lm` fingerprints live in
// `tests/opt_correctness.rs` (Release + Debug≡Release via `opt_sr_correctness.lm`).

e2e_ok!(e2e_alt_option, "examples/guide/alt_option.lm", "10", "42");

e2e_ok!(
    e2e_alt_option_swap_tags,
    "examples/guide/alt_option_swap_tags.lm",
    "7",
    "9"
);

e2e_ok!(
    e2e_alt_result_swap_tags,
    "examples/guide/alt_result_swap_tags.lm",
    "7",
    "9"
);

e2e_ok!(
    e2e_alt_result_return,
    "examples/guide/alt_result_return.lm",
    "6",
    "-1"
);

e2e_ok!(
    e2e_alt_option_return,
    "examples/guide/alt_option_return.lm",
    "10",
    "-1"
);

e2e_ok!(e2e_zero_arg_return, "examples/guide/zero_arg_return.lm", "42");

e2e_ok!(e2e_return_capture, "examples/guide/return_capture.lm", "42", "7");

e2e_ok!(e2e_return_dead, "examples/guide/return_dead.lm", "3", "0", "42");

e2e_ok!(e2e_add, "examples/guide/add.lm", "42");

e2e_ok!(e2e_match, "examples/guide/match.lm", "20");

e2e_ok!(
    e2e_const_patterns,
    "examples/guide/const_patterns.lm",
    "1",
    "2",
    "3",
    "4",
    "5"
);

e2e_ok!(e2e_pure_io_thunk, "examples/guide/pure_io_thunk.lm", "7");

#[test]
fn e2e_map_adt_assoc() {
    // No `instance Hash` → assoc list; still correct after growing past the
    // former SmallMap size threshold (emit is always hash or assoc).
    run_example(
        "examples/guide/map_adt_assoc.lm",
        &["20", "0", "38", "true", "false"],
    );
}

#[test]
fn e2e_map_adt_hash() {
    run_example(
        "examples/guide/map_adt_hash.lm",
        &["20", "0", "38", "true", "false"],
    );
}

#[test]
fn e2e_std_option() {
    // Source-backed `std.option` combinators (inlined from std/option.lm).
    run_example(
        "examples/guide/std_option.lm",
        &["21", "-1", "3", "5", "-1", "true", "true"],
    );
}

#[test]
fn e2e_std_result() {
    run_example(
        "examples/guide/std_result.lm",
        &["42", "-1", "3", "5", "odd", "boom!", "true", "true"],
    );
}

e2e_ok!(e2e_list_for, "examples/guide/list_for.lm", "60");

e2e_ok!(e2e_break, "examples/guide/break.lm", "4");

e2e_ok!(e2e_list_match, "examples/guide/list_match.lm", "0", "7");

e2e_ok!(e2e_to_map, "examples/guide/to_map.lm", "2");

e2e_ok!(e2e_option_match, "examples/guide/option_match.lm", "0", "7");

#[test]
fn e2e_point() {
    run_example(
        "examples/guide/point.lm",
        &["3", "4", "10", "4", "3", "7", "5", "8", "3"],
    );
}

e2e_ok!(e2e_use_math, "examples/guide/use_math.lm", "42", "42");

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

e2e_ok!(e2e_import_as, "examples/guide/import_as.lm", "42", "42");

e2e_ok!(
    e2e_std_string,
    "examples/guide/std_string.lm",
    "Hello",
    "hello",
    "true",
    "a/b/c"
);

e2e_ok!(e2e_use_priv, "examples/guide/use_priv.lm", "42", "42");

e2e_ok!(e2e_priv_sibling_val, "examples/guide/priv_sibling_val.lm", "42");

e2e_ok!(e2e_use_pkg, "examples/guide/use_pkg.lm", "42", "42");

e2e_ok!(e2e_list_hof, "examples/guide/list_hof.lm", "5", "2", "3", "24");

e2e_ok!(
    e2e_list_hof_fn,
    "examples/guide/list_hof_fn.lm",
    "10",
    "30",
    "1",
    "3",
    "6"
);

e2e_ok!(
    e2e_list_concat,
    "examples/guide/list_concat.lm",
    "5",
    "1",
    "5",
    "30"
);

e2e_ok!(e2e_list_pipe, "examples/guide/list_pipe.lm", "3", "6", "10");

e2e_ok!(
    e2e_list_set,
    "examples/guide/list_set.lm",
    "1",
    "99",
    "3",
    "2",
    "3"
);

e2e_ok!(e2e_list_set_alias, "examples/guide/list_set_alias.lm", "2", "99");

e2e_ok!(
    e2e_adt_with_alias,
    "examples/guide/adt_with_alias.lm",
    "1",
    "99",
    "1",
    "99"
);

e2e_ok!(
    e2e_var_fun_reassign,
    "examples/guide/var_fun_reassign.lm",
    "6",
    "6",
    "11",
    "3"
);

e2e_ok!(e2e_with_open_recv, "examples/guide/with_open_recv.lm", "10", "4", "3");

e2e_ok!(
    e2e_shared_product_field,
    "examples/guide/shared_product_field.lm",
    "1",
    "10",
    "7",
    "2"
);

e2e_ok!(
    e2e_take_escape,
    "examples/guide/take_escape.lm",
    "42",
    "1999000"
);

e2e_ok!(e2e_match_guard, "examples/guide/match_guard.lm", "1", "2", "0");

e2e_ok!(e2e_match_cond, "examples/guide/match_cond.lm", "1", "0", "-1");

e2e_ok!(e2e_logic, "examples/guide/logic.lm", "1", "10");

e2e_ok!(e2e_string_ops, "examples/guide/string_ops.lm", "5", "hello", "2");

e2e_ok!(e2e_string_eq, "examples/guide/string_eq.lm", "1", "1", "1", "1.5");

#[test]
fn e2e_string_interp() {
    run_example(
        "examples/guide/string_interp.lm",
        &["hello Lumia", "n=42", "43", "plain", "dollar=$n"],
    );
}

e2e_ok!(e2e_fib, "examples/guide/fib.lm", "55");

e2e_ok!(e2e_char, "examples/guide/char.lm", "A", "1", "1", "Z");

e2e_ok!(e2e_closure, "examples/guide/closure.lm", "42", "11");

e2e_ok!(
    e2e_closure_capture,
    "examples/guide/closure_capture.lm",
    "42",
    "101",
    "42"
);

#[test]
fn e2e_map_ops() {
    run_example(
        "examples/guide/map_ops.lm",
        &[
            "true", "20", "10", "-1", "false", "3", "true", "30", "2", "2", "false", "true",
            "false", "2", "10", "1", "10",
        ],
    );
}

#[test]
fn e2e_set_ops() {
    run_example(
        "examples/guide/set_ops.lm",
        &["3", "true", "false", "3", "2", "false", "true", "3", "true"],
    );
}

e2e_ok!(
    e2e_range_fold,
    "examples/guide/range_fold.lm",
    "499999500000",
    "5050"
);

#[test]
fn e2e_mapset() {
    run_example(
        "examples/guide/mapset.lm",
        &["3", "0", "2", "3", "true", "false", "4"],
    );
}

#[test]
fn e2e_coll_lit() {
    run_example(
        "examples/guide/coll_lit.lm",
        &["0", "3", "true", "20", "0", "3", "true", "false", "3", "1"],
    );
}

#[test]
fn e2e_coll_conv() {
    run_example(
        "examples/guide/coll_conv.lm",
        &["3", "true", "false", "3", "2", "true"],
    );
}

#[test]
fn e2e_set_algebra() {
    run_example(
        "examples/guide/set_algebra.lm",
        &[
            "4", "true", "true", "2", "true", "false", "1", "true", "false",
        ],
    );
}

e2e_ok!(e2e_for_map_set, "examples/guide/for_map_set.lm", "6", "3", "30");

#[test]
fn e2e_range_map() {
    run_example(
        "examples/guide/range_map.lm",
        &["5", "2", "10", "5", "1", "9", "249999500000"],
    );
}

#[test]
fn e2e_range_iota() {
    run_example(
        "examples/guide/range_iota.lm",
        &["1000000", "0", "999999", "2", "10", "3", "3"],
    );
}

e2e_ok!(e2e_fuse_hof, "examples/guide/fuse_hof.lm", "24", "250500");

e2e_ok!(e2e_result_match, "examples/guide/result_match.lm", "5", "-1", "3");

#[test]
fn e2e_list_extras() {
    run_example(
        "examples/guide/list_extras.lm",
        &[
            "false", "true", "4", "4", "4", "1", "20", "true", "false", "true", "false", "2", "-1",
        ],
    );
}

#[test]
fn e2e_prelude_option() {
    run_example("examples/guide/prelude_option.lm", &["10", "-1", "42", "7"]);
}

#[test]
fn e2e_string_more() {
    run_example(
        "examples/guide/string_more.lm",
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
        "examples/guide/map_string_keys.lm",
        &["2", "true", "false", "2", "1", "true", "true", "false"],
    );
}

#[test]
fn e2e_read_stdin() {
    run_example_with_stdin(
        "examples/guide/read_stdin.lm",
        Some("  hi hi there  "),
        &["3", "hi", "2", "true", "true"],
    );
}

#[test]
fn e2e_word_count() {
    run_example_with_stdin(
        "examples/guide/word_count.lm",
        Some("Hello World\nhello there\nWORLD\n"),
        &["hello: 2", "there: 1", "world: 2"],
    );
}

#[test]
fn e2e_list_text() {
    run_example(
        "examples/guide/list_text.lm",
        &[
            "2", "3", "1", "2", "3", "a-b-c", "3", "3", "x", "z", "true", "false", "2", "2",
        ],
    );
}

// Transparent Memo `T_f` is enabled under `--release`; results must match.

#[test]
fn e2e_correctness_fixes() {
    run_example(
        "examples/guide/correctness_fixes.lm",
        &["0", "1", "1", "1", "0", "0", "2", "1.25", "2", "2"],
    );
}

e2e_ok!(
    e2e_scope_shadow,
    "examples/guide/scope_shadow.lm",
    "99",
    "1",
    "1",
    "99",
    "1"
);

e2e_ok!(e2e_result_branch, "examples/guide/result_branch.lm", "7", "-1");

e2e_ok!(
    e2e_result_err_payload,
    "examples/guide/result_err_payload.lm",
    "42",
    "4"
);

e2e_ok!(e2e_for_map_keys, "examples/guide/for_map_keys.lm", "3", "2", "3");

#[test]
fn e2e_contains_poly() {
    run_example(
        "examples/guide/contains_poly.lm",
        &["true", "false", "true", "false"],
    );
}

e2e_ok!(
    e2e_module_val_str,
    "examples/guide/module_val_str.lm",
    "hello",
    "4"
);

e2e_ok!(e2e_for_pair_list, "examples/guide/for_pair_list.lm", "66");

#[test]
fn e2e_for_destructure() {
    // Map + List[(K,V)] both support `for (k, v) in …`.
    run_example("examples/guide/for_destructure.lm", &["33", "18"]);
}

#[test]
fn e2e_gc_roots() {
    // Soft-threshold GC must not free `keep` while junk lists allocate.
    run_example("examples/guide/gc_roots.lm", &["1", "3"]);
}

#[test]
fn e2e_map_hash() {
    run_example(
        "examples/guide/map_hash.lm",
        &[
            "40", "0", "117", "-1", "true", "false", "777", "39", "false", "3", "1",
        ],
    );
}

#[test]
fn e2e_set_hash() {
    run_example(
        "examples/guide/set_hash.lm",
        &[
            "40", "true", "true", "false", "40", "true", "39", "false", "true", "1",
        ],
    );
}

#[test]
fn e2e_sort_by() {
    run_example(
        "examples/guide/sort_by.lm",
        &[
            "1", "1", "3", "5", "5", "4", "3", "20", "10", "30", "apple", "banana", "cherry",
        ],
    );
}

#[test]
fn e2e_tuple_fields() {
    run_example(
        "examples/guide/tuple_fields.lm",
        &["10", "20", "30", "200", "100", "300"],
    );
}

#[test]
fn e2e_let_destructure() {
    // `val (a, b) = p` / nested tuple / product struct pattern.
    run_example(
        "examples/guide/let_destructure.lm",
        &["10", "20", "6", "7", "8", "30"],
    );
}

e2e_ok!(e2e_effect_hof, "examples/guide/effect_hof.lm", "41", "42");

e2e_ok!(e2e_effect_block, "examples/guide/effect_block.lm", "42");

#[test]
fn e2e_nested_match() {
    run_example(
        "examples/guide/nested_match.lm",
        &["7", "99", "1", "2", "1", "42", "1"],
    );
}

e2e_ok!(e2e_assert_ok, "examples/guide/assert_ok.lm", "1");


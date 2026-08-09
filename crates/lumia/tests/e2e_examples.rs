//! Cross-platform e2e: build each example with `lumia` and check stdout.
//!
//! Run: `cargo test -p lumia --test e2e_examples`

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn lumia_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lumia"))
}

fn e2e_out_dir() -> PathBuf {
    // Per-process directory so parallel `cargo test` workers do not clobber
    // each other's executables when stems collide.
    let out_dir = std::env::temp_dir().join(format!("lumia_e2e_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&out_dir);
    out_dir
}

/// Platform executable path under the shared e2e output directory.
fn e2e_exe(stem: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    };
    e2e_out_dir().join(name)
}

fn run_example(rel: &str, expected_lines: &[&str]) {
    run_example_build(rel, None, expected_lines, false);
}

fn run_example_release(rel: &str, expected_lines: &[&str]) {
    run_example_build(rel, None, expected_lines, true);
}

fn run_example_with_stdin(rel: &str, stdin: Option<&str>, expected_lines: &[&str]) {
    run_example_build(rel, stdin, expected_lines, false);
}

fn run_example_build(rel: &str, stdin: Option<&str>, expected_lines: &[&str], release: bool) {
    let root = workspace_root();
    let src = root.join(rel);
    assert!(src.is_file(), "missing example {}", src.display());

    let stem = Path::new(rel)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let exe = e2e_exe(&stem);

    let mut args = vec![
        "build".to_string(),
        src.to_str().unwrap().to_string(),
        "-o".to_string(),
        exe.to_str().unwrap().to_string(),
    ];
    if release {
        args.push("--release".into());
    }
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args(&args)
        .status()
        .expect("spawn lumia build");
    assert!(
        status.success(),
        "lumia build failed for {rel}: {status}"
    );

    let mut cmd = Command::new(&exe);
    let output = if let Some(input) = stdin {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", exe.display()));
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(input.as_bytes())
            .expect("write stdin");
        child.wait_with_output().expect("wait")
    } else {
        cmd.output()
            .unwrap_or_else(|e| panic!("run {}: {e}", exe.display()))
    };
    assert!(
        output.status.success(),
        "{rel} exited {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let got: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        got, expected_lines,
        "{rel}: stdout mismatch\n got: {got:?}\n want: {expected_lines:?}"
    );
}

#[test]
fn e2e_hello() {
    run_example("examples/hello.lm", &["42"]);
}

#[test]
fn e2e_add() {
    run_example("examples/add.lm", &["42"]);
}

#[test]
fn e2e_match() {
    run_example("examples/match.lm", &["20"]);
}

#[test]
fn e2e_const_patterns() {
    run_example("examples/const_patterns.lm", &["1", "2", "3", "4", "5"]);
}

#[test]
fn e2e_poly_id() {
    run_example("examples/poly_id.lm", &["2", "1.5", "hi", "3.5"]);
}

#[test]
fn e2e_poly_inc() {
    // Float monomorphization of `{ x -> x + x }` (not just identity).
    run_example("examples/poly_inc.lm", &["2", "3"]);
}

#[test]
fn e2e_tco_sum() {
    // 2e6 tail calls — overflows without musttail; result = n(n+1)/2.
    run_example("examples/tco_sum.lm", &["2000001000000"]);
}

#[test]
fn e2e_tco_list_sum() {
    // Heap List param + musttail after root_pop; sum of range(0, 2e6) = 0..1999999.
    run_example("examples/tco_list_sum.lm", &["1999999000000"]);
}

#[test]
fn e2e_tco_io_countdown() {
    // IO on base case; recursive arm still musttail (~2e6 frames).
    run_example("examples/tco_io_countdown.lm", &["done", "0"]);
}

#[test]
fn e2e_trait_ord() {
    // `instance Ord for Point` enables lexicographic `<`/`>` on products.
    run_example(
        "examples/trait_ord.lm",
        &["true", "true", "true", "true", "true"],
    );
}

#[test]
fn e2e_trait_show() {
    // Custom Show override; structural `#tag(fields)` for types without a method body.
    run_example("examples/trait_show.lm", &["Point", "#0(9)"]);
}

#[test]
fn e2e_trait_show_method() {
    // UFCS `p.show()` → Show builtin / instance override.
    run_example("examples/trait_show_method.lm", &["Point", "Point"]);
}

#[test]
fn e2e_trait_custom_method() {
    // User trait UFCS → mangled `__ToInt_Point_toInt`.
    run_example("examples/trait_custom_method.lm", &["7", "3"]);
}

#[test]
fn e2e_trait_poly_show() {
    // `{ x -> x.show() }` monomorphized at two Show instances.
    run_example("examples/trait_poly_show.lm", &["P", "B"]);
}

#[test]
fn e2e_trait_poly_method() {
    // `{ x -> x.toInt() }` deferred UFCS + post-mono mangled resolve.
    run_example("examples/trait_poly_method.lm", &["7", "4"]);
}

#[test]
fn e2e_bad_trait_poly_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_trait_poly.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    assert!(!out.status.success(), "missing trait instance should fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("ToInt") || combined.contains("instance"),
        "unexpected diagnostics: {combined}"
    );
}

#[test]
fn e2e_trait_custom_default() {
    run_example("examples/trait_custom_default.lm", &["default"]);
}

#[test]
fn e2e_trait_default_show() {
    run_example("examples/trait_default_show.lm", &["default-show"]);
}

#[test]
fn e2e_trait_eq_ord() {
    run_example(
        "examples/trait_eq_ord.lm",
        &["true", "false", "true", "true", "true"],
    );
}

#[test]
fn e2e_trait_eq_ord_method() {
    // UFCS `p.eq(q)` / `p.less(r)` → Binary Eq/Lt + trait overrides.
    run_example(
        "examples/trait_eq_ord_method.lm",
        &["true", "true", "false"],
    );
}

#[test]
fn e2e_trait_num() {
    run_example("examples/trait_num.lm", &["6", "8", "8", "15"]);
}

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
fn e2e_show_float_adt() {
    run_example("examples/show_float_adt.lm", &["#0(1.5, 2.25)"]);
}

#[test]
fn e2e_tco_even_odd() {
    run_example(
        "examples/tco_even_odd.lm",
        &["true", "false", "false"],
    );
}

#[test]
fn e2e_tco_funref() {
    // FunRef local → directized Call + musttail (2e6 depth).
    run_example(
        "examples/tco_funref.lm",
        &["true", "false", "false"],
    );
}

#[test]
fn e2e_tco_float_sum() {
    // Pure Float musttail — same closed form as Int `tco_sum`.
    run_example("examples/tco_float_sum.lm", &["2000001000000"]);
}

#[test]
fn e2e_poly_add1() {
    run_example("examples/poly_add1.lm", &["2", "2.5"]);
}

#[test]
fn e2e_poly_top_dbl() {
    // Top-level `val dbl` Float site → `dbl$Float` clone.
    run_example("examples/poly_top_dbl.lm", &["2", "3"]);
}

#[test]
fn e2e_poly_bool() {
    run_example("examples/poly_bool.lm", &["1", "true", "false"]);
}

#[test]
fn e2e_poly_str() {
    // String sites → `$String` clone; Bool site → `$Bool`; Int shared body.
    run_example("examples/poly_str.lm", &["[hi]", "ok", "42", "true"]);
}

#[test]
fn e2e_poly_option() {
    run_example("examples/poly_option.lm", &["7", "1.5"]);
}

#[test]
fn e2e_poly_list() {
    run_example("examples/poly_list.lm", &["20", "2.5"]);
}

#[test]
fn e2e_poly_unwrap() {
    run_example(
        "examples/poly_unwrap.lm",
        &["7", "-1", "hi", "no"],
    );
}

#[test]
fn e2e_poly_map_id() {
    run_example(
        "examples/poly_map_id.lm",
        &["2", "true", "2", "true"],
    );
}

#[test]
fn e2e_poly_set_id() {
    run_example(
        "examples/poly_set_id.lm",
        &["3", "true", "2", "true"],
    );
}

#[test]
fn e2e_poly_option_map() {
    // FunRef HOF mono: Option map at Int / Float / String.
    run_example(
        "examples/poly_option_map.lm",
        &["42", "3", "-1", "hi!"],
    );
}

#[test]
fn e2e_poly_option_and_then() {
    run_example(
        "examples/poly_option_and_then.lm",
        &["5", "-1", "-2"],
    );
}

#[test]
fn e2e_poly_result_map() {
    run_example(
        "examples/poly_result_map.lm",
        &["42", "3", "boom"],
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

#[test]
fn e2e_small_list_local() {
    // Non-escaping small listOf → stack LitList; len/get still work.
    run_example("examples/small_list_local.lm", &["3", "10", "30", "60"]);
}

#[test]
fn e2e_small_map_local() {
    run_example(
        "examples/small_map_local.lm",
        &["3", "true", "10", "30"],
    );
}

#[test]
fn e2e_small_set_local() {
    run_example("examples/small_set_local.lm", &["3", "true", "false"]);
}

#[test]
fn e2e_pe_list_len_get() {
    // Same output as small_list_local; ListLen/ListGet folded at opt L0 when possible.
    run_example("examples/pe_list_len_get.lm", &["3", "10", "30", "60"]);
}

#[test]
fn e2e_pe_adt_field() {
    run_example("examples/pe_adt_field.lm", &["10", "20", "30"]);
}

#[test]
fn e2e_pe_map_contains() {
    // Const-fold mapOf/setOf → len / contains (memo L0).
    run_example(
        "examples/pe_map_contains.lm",
        &["3", "true", "false", "3", "true", "false"],
    );
}

#[test]
fn e2e_escape_pure_len() {
    // Pure len callee must not force list escape → LitList still works.
    run_example("examples/escape_pure_len.lm", &["3", "20"]);
}

#[test]
fn e2e_small_adt_local() {
    // Non-escaping product via non-capturing field getters → LitAdt.
    run_example("examples/small_adt_local.lm", &["10", "20", "30"]);
}

#[test]
fn e2e_list_for() {
    run_example("examples/list_for.lm", &["60"]);
}

#[test]
fn e2e_break() {
    run_example("examples/break.lm", &["4"]);
}

#[test]
fn e2e_list_match() {
    run_example("examples/list_match.lm", &["0", "7"]);
}

#[test]
fn e2e_to_map() {
    run_example("examples/to_map.lm", &["2"]);
}

#[test]
fn e2e_option_match() {
    run_example("examples/option_match.lm", &["0", "7"]);
}

#[test]
fn e2e_point() {
    run_example(
        "examples/point.lm",
        &["3", "4", "10", "4", "3", "7", "5", "8", "3"],
    );
}

#[test]
fn e2e_use_math() {
    run_example("examples/use_math.lm", &["42", "42"]);
}

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

#[test]
fn e2e_import_as() {
    run_example("examples/import_as.lm", &["42", "42"]);
}

#[test]
fn e2e_bad_import_as_original_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_import_as_original.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    assert!(!out.status.success(), "original name after `as` should fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("private or not imported") || combined.contains("`add`"),
        "unexpected diagnostics: {combined}"
    );
}

#[test]
fn e2e_use_priv() {
    run_example("examples/use_priv.lm", &["42", "42"]);
}

#[test]
fn e2e_use_pkg() {
    run_example("examples/use_pkg.lm", &["42", "42"]);
}

#[test]
fn e2e_list_hof() {
    run_example("examples/list_hof.lm", &["5", "2", "3", "24"]);
}

#[test]
fn e2e_list_hof_fn() {
    run_example("examples/list_hof_fn.lm", &["10", "30", "1", "3", "6"]);
}

#[test]
fn e2e_list_concat() {
    run_example("examples/list_concat.lm", &["5", "1", "5", "30"]);
}

#[test]
fn e2e_list_pipe() {
    run_example("examples/list_pipe.lm", &["3", "6", "10"]);
}

#[test]
fn e2e_list_set() {
    run_example("examples/list_set.lm", &["1", "99", "3", "2", "3"]);
}

#[test]
fn e2e_match_guard() {
    run_example("examples/match_guard.lm", &["1", "2", "0"]);
}

#[test]
fn e2e_match_cond() {
    run_example("examples/match_cond.lm", &["1", "0", "-1"]);
}

#[test]
fn e2e_logic() {
    run_example("examples/logic.lm", &["1", "10"]);
}

#[test]
fn e2e_string_ops() {
    run_example("examples/string_ops.lm", &["5", "hello", "2"]);
}

#[test]
fn e2e_string_eq() {
    run_example("examples/string_eq.lm", &["1", "1", "1", "1.5"]);
}

#[test]
fn e2e_string_interp() {
    run_example(
        "examples/string_interp.lm",
        &["hello Lumia", "n=42", "43", "plain", "dollar=$n"],
    );
}

#[test]
fn e2e_fib() {
    run_example("examples/fib.lm", &["55"]);
}

#[test]
fn e2e_char() {
    run_example("examples/char.lm", &["A", "1", "1", "Z"]);
}

#[test]
fn e2e_float_ops() {
    run_example(
        "examples/float_ops.lm",
        &["3.75", "6", "1", "-1.5", "4"],
    );
}

#[test]
fn e2e_float_map_keys() {
    // ±0 collide; NaN never hits contains (matches IEEE ==).
    run_example(
        "examples/float_map_keys.lm",
        &["true", "1", "false", "0", "true", "true"],
    );
}

#[test]
fn e2e_float_struct_eq() {
    // List/Option/Map Float payloads: ±0 equal; NaN ≠ NaN (DESIGN §2.1).
    run_example(
        "examples/float_struct_eq.lm",
        &["1", "0", "1", "0", "1", "0"],
    );
}

#[test]
fn e2e_closure() {
    run_example("examples/closure.lm", &["42", "11"]);
}

#[test]
fn e2e_closure_capture() {
    run_example("examples/closure_capture.lm", &["42", "101", "42"]);
}

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

#[test]
fn e2e_range_fold() {
    run_example("examples/range_fold.lm", &["499999500000", "5050"]);
}

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
        &["4", "true", "true", "2", "true", "false", "1", "true", "false"],
    );
}

#[test]
fn e2e_for_map_set() {
    run_example("examples/for_map_set.lm", &["6", "3", "30"]);
}

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

#[test]
fn e2e_fuse_hof() {
    run_example("examples/fuse_hof.lm", &["24", "250500"]);
}

#[test]
fn e2e_result_match() {
    run_example("examples/result_match.lm", &["5", "-1", "3"]);
}

#[test]
fn e2e_list_extras() {
    run_example(
        "examples/list_extras.lm",
        &[
            "false", "true", "4", "4", "4", "1", "20", "true", "false", "true", "false", "2",
            "-1",
        ],
    );
}

#[test]
fn e2e_prelude_option() {
    run_example(
        "examples/prelude_option.lm",
        &["10", "-1", "42", "7"],
    );
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

#[test]
fn e2e_memo_l2_release() {
    // Transparent Memo L2 is enabled under `--release`; results must match.
    run_example_release(
        "examples/memo_l2.lm",
        &["2646700", "2646700", "285"],
    );
}

#[test]
fn e2e_memo_l0l1() {
    run_example("examples/memo_l0l1.lm", &["42", "42", "65"]);
}

#[test]
fn e2e_correctness_fixes() {
    run_example(
        "examples/correctness_fixes.lm",
        &["0", "1", "1", "1", "0", "0", "2", "1.25", "2", "2"],
    );
}

#[test]
fn e2e_scope_shadow() {
    run_example("examples/scope_shadow.lm", &["99", "1", "1", "99", "1"]);
}

#[test]
fn e2e_result_branch() {
    run_example("examples/result_branch.lm", &["7", "-1"]);
}

#[test]
fn e2e_result_err_payload() {
    run_example("examples/result_err_payload.lm", &["42", "4"]);
}

#[test]
fn e2e_for_map_keys() {
    run_example("examples/for_map_keys.lm", &["3", "2", "3"]);
}

#[test]
fn e2e_contains_poly() {
    run_example(
        "examples/contains_poly.lm",
        &["true", "false", "true", "false"],
    );
}

#[test]
fn e2e_module_val_str() {
    run_example("examples/module_val_str.lm", &["hello", "4"]);
}

#[test]
fn e2e_for_pair_list() {
    run_example("examples/for_pair_list.lm", &["66"]);
}

#[test]
fn e2e_hof_float_to_int() {
    run_example("examples/hof_float_to_int.lm", &["1", "2"]);
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
            "1", "1", "3", "5", "5", "4", "3", "20", "10", "30", "apple", "banana",
            "cherry",
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
fn e2e_effect_hof() {
    run_example("examples/effect_hof.lm", &["41", "42"]);
}

#[test]
fn e2e_effect_block() {
    run_example("examples/effect_block.lm", &["42"]);
}

#[test]
fn e2e_nested_match() {
    run_example(
        "examples/nested_match.lm",
        &["7", "99", "1", "2", "1", "42", "1"],
    );
}

#[test]
fn e2e_bad_import_priv_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_import_priv.lm");
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .status()
        .expect("spawn lumia check");
    assert!(
        !status.success(),
        "priv import should fail type/check"
    );
}

#[test]
fn e2e_priv_leak_rejected() {
    let root = workspace_root();
    let src = root.join("examples/priv_leak_test.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check priv_leak_test");
    assert!(
        !out.status.success(),
        "priv helper must not be visible via unrelated import"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("private") || combined.contains("unbound") || combined.contains("helper"),
        "expected priv/visibility error, got: {combined}"
    );
}

#[test]
fn e2e_bad_nested_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_nested_match.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    assert!(
        !out.status.success(),
        "nested non-exhaustive match should fail check"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{err}{}", String::from_utf8_lossy(&out.stdout));
    assert!(
        combined.contains("non-exhaustive"),
        "expected non-exhaustive error, got: {combined}"
    );
    assert!(
        combined.contains("bad_nested_match.lm:") && combined.contains(": lower:"),
        "expected located diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_bad_int_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_int_match.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    assert!(!out.status.success(), "int literal match should fail check");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("non-exhaustive") && combined.contains("Int"),
        "unexpected diagnostics: {combined}"
    );
    assert!(
        combined.contains("bad_int_match.lm:") && combined.contains("^"),
        "expected located diagnostic with caret, got: {combined}"
    );
}

#[test]
fn e2e_bad_empty_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_empty_match.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    assert!(!out.status.success(), "empty match should fail check");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("non-exhaustive"),
        "unexpected diagnostics: {combined}"
    );
}

#[test]
fn e2e_bad_guard_only_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_guard_only.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    assert!(!out.status.success(), "guard-only match should fail check");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("non-exhaustive"),
        "unexpected diagnostics: {combined}"
    );
}

#[test]
fn e2e_bad_list_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_list_match.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    assert!(!out.status.success(), "partial list match should fail check");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        combined.contains("non-exhaustive") && combined.contains("List"),
        "unexpected diagnostics: {combined}"
    );
    assert!(
        combined.contains("bad_list_match.lm:"),
        "expected located diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_assert_ok() {
    run_example("examples/assert_ok.lm", &["1"]);
}

#[test]
fn e2e_bad_assert_aborts() {
    let root = workspace_root();
    let src = root.join("examples/bad_assert.lm");
    let bin = e2e_exe("bad_assert");
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            bin.to_str().unwrap(),
        ])
        .status()
        .expect("build bad_assert");
    assert!(status.success(), "bad_assert should compile");
    let run = Command::new(&bin).output().expect("run bad_assert");
    assert!(
        !run.status.success(),
        "assert(false) should abort the process"
    );
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        err.contains("assert failed") && err.contains("bad_assert.lm:"),
        "unexpected stderr: {err}"
    );
}

#[test]
fn e2e_bad_import_type_points_at_dep() {
    let root = workspace_root();
    let src = root.join("examples/bad_import_type.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_import_type");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        err
    );
    assert!(
        combined.contains("bad_dep.lm:") && combined.contains("type mismatch"),
        "expected dep-file diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_bad_dep_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_dep.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_dep");
    assert!(!out.status.success(), "bad_dep should fail type check");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("bad_dep.lm:") && combined.contains("type mismatch"),
        "expected located type mismatch, got: {combined}"
    );
}

fn run_example_trust_foreign_pure(rel: &str, expected_lines: &[&str]) {
    let root = workspace_root();
    let src = root.join(rel);
    let stem = Path::new(rel)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let exe = e2e_exe(&stem);
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args([
            "build",
            "--trust-foreign-pure",
            src.to_str().unwrap(),
            "-o",
            exe.to_str().unwrap(),
        ])
        .status()
        .expect("spawn lumia build");
    assert!(
        status.success(),
        "lumia build failed for {rel}: {status}"
    );
    let output = Command::new(&exe)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", exe.display()));
    assert!(
        output.status.success(),
        "{rel} exited {}: stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let got: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        got, expected_lines,
        "{rel}: stdout mismatch\n got: {got:?}\n want: {expected_lines:?}"
    );
}

#[test]
fn e2e_ffi_abs() {
    run_example_trust_foreign_pure("examples/ffi_abs.lm", &["42", "7"]);
}

#[test]
fn e2e_ffi_strlen() {
    run_example_trust_foreign_pure("examples/ffi_strlen.lm", &["5", "0"]);
}

#[test]
fn e2e_ffi_getenv() {
    run_example("examples/ffi_getenv.lm", &["true", "0"]);
}

#[test]
fn e2e_bad_foreign_pure_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_foreign_pure.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_foreign_pure");
    assert!(
        !out.status.success(),
        "bad_foreign_pure should fail without --trust-foreign-pure"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("trust-foreign-pure") && combined.contains("pure"),
        "expected trust-foreign-pure diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_use_path_dep() {
    run_example("examples/use_path_dep.lm", &["42", "42"]);
}

#[test]
fn e2e_par_map() {
    // Auto-parallel is on by default (no --parallel flag).
    run_example("examples/par_map.lm", &["200", "0", "398"]);
}

#[test]
fn e2e_par_fold() {
    // sum 0..99 = 4950; FunRef + capture-free lambda both ListParFold-safe.
    run_example("examples/par_fold.lm", &["4950", "4950"]);
}

#[test]
fn e2e_par_map_fn() {
    run_example("examples/par_map_fn.lm", &["50", "0", "98"]);
}

#[test]
fn e2e_par_map_toplevel_lam() {
    // `{ x -> double(x) }` with top-level `double` stays ListParMap-safe.
    run_example("examples/par_map_toplevel_lam.lm", &["50", "0", "98"]);
}

#[test]
fn e2e_par_map_capture() {
    // Capturing closure stays sequential; still correct under auto-parallel.
    run_example("examples/par_map_capture.lm", &["5", "10", "14"]);
}

#[test]
fn e2e_unknown_std_module_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_std_import.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_std_import");
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("unknown standard module") || combined.contains("not exported"),
        "expected std allowlist error, got: {combined}"
    );
}

#[test]
fn e2e_bad_field_proj_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_field_proj.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_field_proj");
    assert!(!out.status.success(), "wrong product field must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("expects type")
            || combined.contains("field projection")
            || combined.contains("cannot resolve"),
        "expected field-type error, got: {combined}"
    );
}

#[test]
fn e2e_unknown_trait_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_trait.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_trait");
    assert!(!out.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("unknown trait") || combined.contains("NotATrait"),
        "expected unknown-trait error, got: {combined}"
    );
}

#[test]
fn e2e_int_literal_overflow_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_int_overflow.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_int_overflow");
    assert!(!out.status.success(), "overflowing Int literal must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("out of range") || combined.contains("integer literal"),
        "expected overflow diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_bad_val_assign_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_val_assign.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_val_assign");
    assert!(!out.status.success(), "assign to val must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("immutable") || combined.contains("cannot assign"),
        "expected immutability error, got: {combined}"
    );
}

#[test]
fn e2e_bad_struct_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_struct_match.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_struct_match");
    assert!(!out.status.success(), "Point pattern on Rect must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("expects type") || combined.contains("Point") || combined.contains("Rect"),
        "expected product mismatch error, got: {combined}"
    );
}

#[test]
fn e2e_bad_ok_arity_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_ok_arity.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_ok_arity");
    assert!(!out.status.success(), "Ok() vs Ok(x) arity must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("expects") || combined.contains("field") || combined.contains("lower"),
        "expected arity error, got: {combined}"
    );
}

#[test]
fn e2e_bad_struct_field_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_struct_field_match.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_struct_field_match");
    assert!(!out.status.success(), "unknown struct field in pattern must fail");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("unknown field") || combined.contains('z'),
        "expected unknown-field diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_bad_par_map_io_demoted() {
    // Effectful FunRef map is demoted to sequential — still type-checks.
    let root = workspace_root();
    let src = root.join("examples/bad_par_map_io.lm");
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("check bad_par_map_io");
    assert!(
        out.status.success(),
        "effectful map should demote, not fail: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

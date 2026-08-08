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
    let out_dir = std::env::temp_dir().join("lumia_e2e");
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
    run_example("examples/hello.lumia", &["42"]);
}

#[test]
fn e2e_add() {
    run_example("examples/add.lumia", &["42"]);
}

#[test]
fn e2e_match() {
    run_example("examples/match.lumia", &["20"]);
}

#[test]
fn e2e_list_for() {
    run_example("examples/list_for.lumia", &["60"]);
}

#[test]
fn e2e_break() {
    run_example("examples/break.lumia", &["4"]);
}

#[test]
fn e2e_list_match() {
    run_example("examples/list_match.lumia", &["0", "7"]);
}

#[test]
fn e2e_to_map() {
    run_example("examples/to_map.lumia", &["2"]);
}

#[test]
fn e2e_option_match() {
    run_example("examples/option_match.lumia", &["0", "7"]);
}

#[test]
fn e2e_point() {
    run_example(
        "examples/point.lumia",
        &["3", "4", "10", "4", "3", "7", "5", "8", "3"],
    );
}

#[test]
fn e2e_use_math() {
    run_example("examples/use_math.lumia", &["42", "42"]);
}

#[test]
fn e2e_use_priv() {
    run_example("examples/use_priv.lumia", &["42", "42"]);
}

#[test]
fn e2e_use_pkg() {
    run_example("examples/use_pkg.lumia", &["42", "42"]);
}

#[test]
fn e2e_list_hof() {
    run_example("examples/list_hof.lumia", &["5", "2", "3", "24"]);
}

#[test]
fn e2e_list_hof_fn() {
    run_example("examples/list_hof_fn.lumia", &["10", "30", "1", "3", "6"]);
}

#[test]
fn e2e_list_concat() {
    run_example("examples/list_concat.lumia", &["5", "1", "5", "30"]);
}

#[test]
fn e2e_list_pipe() {
    run_example("examples/list_pipe.lumia", &["3", "6", "10"]);
}

#[test]
fn e2e_list_set() {
    run_example("examples/list_set.lumia", &["1", "99", "3", "2", "3"]);
}

#[test]
fn e2e_match_guard() {
    run_example("examples/match_guard.lumia", &["1", "2", "0"]);
}

#[test]
fn e2e_match_cond() {
    run_example("examples/match_cond.lumia", &["1", "0", "-1"]);
}

#[test]
fn e2e_logic() {
    run_example("examples/logic.lumia", &["1", "10"]);
}

#[test]
fn e2e_string_ops() {
    run_example("examples/string_ops.lumia", &["5", "hello", "2"]);
}

#[test]
fn e2e_string_eq() {
    run_example("examples/string_eq.lumia", &["1", "1", "1", "1.5"]);
}

#[test]
fn e2e_string_interp() {
    run_example(
        "examples/string_interp.lumia",
        &["hello Lumia", "n=42", "43", "plain", "dollar=$n"],
    );
}

#[test]
fn e2e_fib() {
    run_example("examples/fib.lumia", &["55"]);
}

#[test]
fn e2e_char() {
    run_example("examples/char.lumia", &["A", "1", "1", "Z"]);
}

#[test]
fn e2e_float_ops() {
    run_example("examples/float_ops.lumia", &["3.75", "6", "1", "-1.5"]);
}

#[test]
fn e2e_closure() {
    run_example("examples/closure.lumia", &["42", "11"]);
}

#[test]
fn e2e_closure_capture() {
    run_example("examples/closure_capture.lumia", &["42", "101", "42"]);
}

#[test]
fn e2e_map_ops() {
    run_example(
        "examples/map_ops.lumia",
        &[
            "true", "20", "10", "-1", "false", "3", "true", "30", "2", "2", "false", "true",
            "false", "2", "10", "1", "10",
        ],
    );
}

#[test]
fn e2e_set_ops() {
    run_example(
        "examples/set_ops.lumia",
        &["3", "true", "false", "3", "2", "false", "true", "3", "true"],
    );
}

#[test]
fn e2e_range_fold() {
    run_example("examples/range_fold.lumia", &["499999500000", "5050"]);
}

#[test]
fn e2e_mapset() {
    run_example(
        "examples/mapset.lumia",
        &["3", "0", "2", "3", "true", "false", "4"],
    );
}

#[test]
fn e2e_coll_lit() {
    run_example(
        "examples/coll_lit.lumia",
        &["0", "3", "true", "20", "0", "3", "true", "false", "3", "1"],
    );
}

#[test]
fn e2e_coll_conv() {
    run_example(
        "examples/coll_conv.lumia",
        &["3", "true", "false", "3", "2", "true"],
    );
}

#[test]
fn e2e_set_algebra() {
    run_example(
        "examples/set_algebra.lumia",
        &["4", "true", "true", "2", "true", "false", "1", "true", "false"],
    );
}

#[test]
fn e2e_for_map_set() {
    run_example("examples/for_map_set.lumia", &["6", "3", "30"]);
}

#[test]
fn e2e_range_map() {
    run_example(
        "examples/range_map.lumia",
        &["5", "2", "10", "5", "1", "9", "249999500000"],
    );
}

#[test]
fn e2e_range_iota() {
    run_example(
        "examples/range_iota.lumia",
        &["1000000", "0", "999999", "2", "10", "3", "3"],
    );
}

#[test]
fn e2e_fuse_hof() {
    run_example("examples/fuse_hof.lumia", &["24", "250500"]);
}

#[test]
fn e2e_result_match() {
    run_example("examples/result_match.lumia", &["5", "-1", "3"]);
}

#[test]
fn e2e_list_extras() {
    run_example(
        "examples/list_extras.lumia",
        &[
            "false", "true", "4", "4", "4", "1", "20", "true", "false", "true", "false", "2",
            "-1",
        ],
    );
}

#[test]
fn e2e_prelude_option() {
    run_example(
        "examples/prelude_option.lumia",
        &["10", "-1", "42", "7"],
    );
}

#[test]
fn e2e_string_more() {
    run_example(
        "examples/string_more.lumia",
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
        "examples/map_string_keys.lumia",
        &["2", "true", "false", "2", "1", "true", "true", "false"],
    );
}

#[test]
fn e2e_read_stdin() {
    run_example_with_stdin(
        "examples/read_stdin.lumia",
        Some("  hi hi there  "),
        &["3", "hi", "2", "true", "true"],
    );
}

#[test]
fn e2e_word_count() {
    run_example_with_stdin(
        "examples/word_count.lumia",
        Some("Hello World\nhello there\nWORLD\n"),
        &["hello: 2", "there: 1", "world: 2"],
    );
}

#[test]
fn e2e_list_text() {
    run_example(
        "examples/list_text.lumia",
        &[
            "2", "3", "1", "2", "3", "a-b-c", "3", "3", "x", "z", "true", "false", "2", "2",
        ],
    );
}

#[test]
fn e2e_memo_l2_release() {
    // Transparent Memo L2 is enabled under `--release`; results must match.
    run_example_release(
        "examples/memo_l2.lumia",
        &["2646700", "2646700", "285"],
    );
}

#[test]
fn e2e_memo_l0l1() {
    run_example("examples/memo_l0l1.lumia", &["42", "42", "65"]);
}

#[test]
fn e2e_correctness_fixes() {
    run_example(
        "examples/correctness_fixes.lumia",
        &["0", "1", "1", "1", "0", "0", "2", "1.25", "2", "2"],
    );
}

#[test]
fn e2e_scope_shadow() {
    run_example("examples/scope_shadow.lumia", &["99", "1", "1", "99", "1"]);
}

#[test]
fn e2e_result_branch() {
    run_example("examples/result_branch.lumia", &["7", "-1"]);
}

#[test]
fn e2e_result_err_payload() {
    run_example("examples/result_err_payload.lumia", &["42", "4"]);
}

#[test]
fn e2e_for_map_keys() {
    run_example("examples/for_map_keys.lumia", &["3", "2", "3"]);
}

#[test]
fn e2e_contains_poly() {
    run_example(
        "examples/contains_poly.lumia",
        &["true", "false", "true", "false"],
    );
}

#[test]
fn e2e_module_val_str() {
    run_example("examples/module_val_str.lumia", &["hello", "4"]);
}

#[test]
fn e2e_for_pair_list() {
    run_example("examples/for_pair_list.lumia", &["66"]);
}

#[test]
fn e2e_hof_float_to_int() {
    run_example("examples/hof_float_to_int.lumia", &["1", "2"]);
}

#[test]
fn e2e_gc_roots() {
    // Soft-threshold GC must not free `keep` while junk lists allocate.
    run_example("examples/gc_roots.lumia", &["1", "3"]);
}

#[test]
fn e2e_map_hash() {
    run_example(
        "examples/map_hash.lumia",
        &[
            "40", "0", "117", "-1", "true", "false", "777", "39", "false", "3", "1",
        ],
    );
}

#[test]
fn e2e_set_hash() {
    run_example(
        "examples/set_hash.lumia",
        &[
            "40", "true", "true", "false", "40", "true", "39", "false", "true", "1",
        ],
    );
}

#[test]
fn e2e_sort_by() {
    run_example(
        "examples/sort_by.lumia",
        &[
            "1", "1", "3", "5", "5", "4", "3", "20", "10", "30", "apple", "banana",
            "cherry",
        ],
    );
}

#[test]
fn e2e_tuple_fields() {
    run_example(
        "examples/tuple_fields.lumia",
        &["10", "20", "30", "200", "100", "300"],
    );
}

#[test]
fn e2e_effect_hof() {
    run_example("examples/effect_hof.lumia", &["41", "42"]);
}

#[test]
fn e2e_effect_block() {
    run_example("examples/effect_block.lumia", &["42"]);
}

#[test]
fn e2e_nested_match() {
    run_example(
        "examples/nested_match.lumia",
        &["7", "99", "1", "2", "1", "42", "1"],
    );
}

#[test]
fn e2e_bad_import_priv_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_import_priv.lumia");
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
    let src = root.join("examples/priv_leak_test.lumia");
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
    let src = root.join("examples/bad_nested_match.lumia");
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
        combined.contains("bad_nested_match.lumia:") && combined.contains(": lower:"),
        "expected located diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_bad_int_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_int_match.lumia");
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
        combined.contains("bad_int_match.lumia:") && combined.contains("^"),
        "expected located diagnostic with caret, got: {combined}"
    );
}

#[test]
fn e2e_bad_list_match_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_list_match.lumia");
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
        combined.contains("bad_list_match.lumia:"),
        "expected located diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_assert_ok() {
    run_example("examples/assert_ok.lumia", &["1"]);
}

#[test]
fn e2e_bad_assert_aborts() {
    let root = workspace_root();
    let src = root.join("examples/bad_assert.lumia");
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
        err.contains("assert failed") && err.contains("bad_assert.lumia:"),
        "unexpected stderr: {err}"
    );
}

#[test]
fn e2e_bad_import_type_points_at_dep() {
    let root = workspace_root();
    let src = root.join("examples/bad_import_type.lumia");
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
        combined.contains("bad_dep.lumia:") && combined.contains("type mismatch"),
        "expected dep-file diagnostic, got: {combined}"
    );
}

#[test]
fn e2e_bad_dep_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_dep.lumia");
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
        combined.contains("bad_dep.lumia:") && combined.contains("type mismatch"),
        "expected located type mismatch, got: {combined}"
    );
}

#[test]
fn e2e_ffi_abs() {
    run_example("examples/ffi_abs.lumia", &["42", "7"]);
}

#[test]
fn e2e_ffi_strlen() {
    run_example("examples/ffi_strlen.lumia", &["5", "0"]);
}

#[test]
fn e2e_ffi_getenv() {
    run_example("examples/ffi_getenv.lumia", &["true", "0"]);
}

#[test]
fn e2e_use_path_dep() {
    run_example("examples/use_path_dep.lumia", &["42", "42"]);
}

#[test]
fn e2e_par_map() {
    let root = workspace_root();
    let src = root.join("examples/par_map.lumia");
    let exe = e2e_exe("par_map");
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args([
            "build",
            "--parallel",
            src.to_str().unwrap(),
            "-o",
            exe.to_str().unwrap(),
        ])
        .status()
        .expect("build par_map");
    assert!(status.success(), "par_map build failed");
    let output = Command::new(&exe).output().expect("run par_map");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let got: Vec<&str> = stdout.lines().collect();
    assert_eq!(got, ["200", "0", "398"]);
}

#[test]
fn e2e_par_map_fn() {
    let root = workspace_root();
    let src = root.join("examples/par_map_fn.lumia");
    let exe = e2e_exe("par_map_fn");
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args([
            "build",
            "--parallel",
            src.to_str().unwrap(),
            "-o",
            exe.to_str().unwrap(),
        ])
        .status()
        .expect("build par_map_fn");
    assert!(status.success(), "par_map_fn build failed");
    let output = Command::new(&exe).output().expect("run par_map_fn");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let got: Vec<&str> = stdout.lines().collect();
    assert_eq!(got, ["50", "0", "98"]);
}

#[test]
fn e2e_par_map_capture() {
    let root = workspace_root();
    let src = root.join("examples/par_map_capture.lumia");
    let exe = e2e_exe("par_map_capture");
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args([
            "build",
            "--parallel",
            src.to_str().unwrap(),
            "-o",
            exe.to_str().unwrap(),
        ])
        .status()
        .expect("build par_map_capture");
    assert!(status.success(), "par_map_capture build failed");
    let output = Command::new(&exe).output().expect("run par_map_capture");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let got: Vec<&str> = stdout.lines().collect();
    assert_eq!(got, ["5", "10", "14"]);
}

#[test]
fn e2e_unknown_std_module_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_std_import.lumia");
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
    let src = root.join("examples/bad_field_proj.lumia");
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
fn e2e_trait_keyword_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_trait.lumia");
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
        combined.contains("not implemented") || combined.contains("reserved"),
        "expected reserved-keyword error, got: {combined}"
    );
}

#[test]
fn e2e_int_literal_overflow_rejected() {
    let root = workspace_root();
    let src = root.join("examples/bad_int_overflow.lumia");
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
    let src = root.join("examples/bad_val_assign.lumia");
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
    let src = root.join("examples/bad_struct_match.lumia");
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
    let src = root.join("examples/bad_ok_arity.lumia");
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

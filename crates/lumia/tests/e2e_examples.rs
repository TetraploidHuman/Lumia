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

    let out_dir = std::env::temp_dir().join("lumia_e2e");
    let _ = std::fs::create_dir_all(&out_dir);
    let stem = Path::new(rel)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let exe_name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.clone()
    };
    let exe = out_dir.join(exe_name);

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
            "1", "20", "10", "-1", "0", "3", "1", "30", "2", "2", "0", "1", "0", "2", "10", "1",
            "10",
        ],
    );
}

#[test]
fn e2e_set_ops() {
    run_example(
        "examples/set_ops.lumia",
        &["3", "1", "0", "3", "2", "0", "1", "3", "1"],
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
        &["3", "0", "2", "3", "1", "0", "4"],
    );
}

#[test]
fn e2e_coll_lit() {
    run_example(
        "examples/coll_lit.lumia",
        &["0", "3", "1", "20", "0", "3", "1", "0", "3", "1"],
    );
}

#[test]
fn e2e_coll_conv() {
    run_example(
        "examples/coll_conv.lumia",
        &["3", "1", "0", "3", "2", "1"],
    );
}

#[test]
fn e2e_set_algebra() {
    run_example(
        "examples/set_algebra.lumia",
        &["4", "1", "1", "2", "1", "0", "1", "1", "0"],
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
        &["0", "1", "4", "4", "4", "1", "20", "1", "0", "1", "0", "2", "-1"],
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
        &["2", "1", "0", "2", "1", "1", "1", "0"],
    );
}

#[test]
fn e2e_read_stdin() {
    run_example_with_stdin(
        "examples/read_stdin.lumia",
        Some("  hi hi there  "),
        &["3", "hi", "2", "1", "1"],
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
            "2", "3", "1", "2", "3", "a-b-c", "3", "3", "x", "z", "1", "0", "2", "2",
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
fn e2e_gc_roots() {
    // Soft-threshold GC must not free `keep` while junk lists allocate.
    run_example("examples/gc_roots.lumia", &["1", "3"]);
}

#[test]
fn e2e_map_hash() {
    run_example(
        "examples/map_hash.lumia",
        &["40", "0", "117", "-1", "1", "0", "777", "39", "0", "3", "1"],
    );
}

#[test]
fn e2e_set_hash() {
    run_example(
        "examples/set_hash.lumia",
        &["40", "1", "1", "0", "40", "1", "39", "0", "1", "1"],
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
    let out_dir = std::env::temp_dir().join("lumia_e2e");
    let _ = std::fs::create_dir_all(&out_dir);
    let bin = out_dir.join("bad_assert");
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
    run_example("examples/ffi_getenv.lumia", &["1", "0"]);
}

#[test]
fn e2e_use_path_dep() {
    run_example("examples/use_path_dep.lumia", &["42", "42"]);
}

#[test]
fn e2e_par_map() {
    let root = workspace_root();
    let src = root.join("examples/par_map.lumia");
    let out_dir = std::env::temp_dir().join("lumia_e2e");
    let _ = std::fs::create_dir_all(&out_dir);
    let exe = out_dir.join("par_map");
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
    let out_dir = std::env::temp_dir().join("lumia_e2e");
    let _ = std::fs::create_dir_all(&out_dir);
    let exe = out_dir.join("par_map_fn");
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
    let out_dir = std::env::temp_dir().join("lumia_e2e");
    let _ = std::fs::create_dir_all(&out_dir);
    let exe = out_dir.join("par_map_capture");
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

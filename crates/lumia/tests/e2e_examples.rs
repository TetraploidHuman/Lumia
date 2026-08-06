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

    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["build", src.to_str().unwrap(), "-o", exe.to_str().unwrap()])
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
fn e2e_match_guard() {
    run_example("examples/match_guard.lumia", &["1", "2", "0"]);
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

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../common/mod.rs"]
mod common;

pub(crate) use common::{lumia_bin, workspace_root};

pub(crate) fn e2e_out_dir() -> PathBuf {
    // Per-process directory so parallel `cargo test` workers do not clobber
    // each other's executables when stems collide.
    let out_dir = std::env::temp_dir().join(format!("lumia_e2e_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&out_dir);
    out_dir
}

/// Platform executable path under the shared e2e output directory.
pub(crate) fn e2e_exe(stem: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    };
    e2e_out_dir().join(name)
}

pub(crate) fn run_example(rel: &str, expected_lines: &[&str]) {
    run_example_build(rel, None, expected_lines, false, &[]);
}

pub(crate) fn run_example_release(rel: &str, expected_lines: &[&str]) {
    run_example_build(rel, None, expected_lines, true, &[]);
}

pub(crate) fn run_example_with_stdin(rel: &str, stdin: Option<&str>, expected_lines: &[&str]) {
    run_example_build(rel, stdin, expected_lines, false, &[]);
}

pub(crate) fn run_example_trust_foreign_pure(rel: &str, expected_lines: &[&str]) {
    run_example_build(rel, None, expected_lines, false, &["--trust-foreign-pure"]);
}

pub(crate) fn run_example_build(
    rel: &str,
    stdin: Option<&str>,
    expected_lines: &[&str],
    release: bool,
    extra_args: &[&str],
) {
    run_example_build_env(rel, stdin, expected_lines, release, extra_args, &[]);
}

/// Like [`run_example_build`], but sets env vars on the **compiled program** (not the builder).
pub(crate) fn run_example_build_env(
    rel: &str,
    stdin: Option<&str>,
    expected_lines: &[&str],
    release: bool,
    extra_args: &[&str],
    run_env: &[(&str, &str)],
) {
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
    for a in extra_args {
        args.push((*a).to_string());
    }
    let status = Command::new(lumia_bin())
        .current_dir(&root)
        .args(&args)
        .status()
        .expect("spawn lumia build");
    assert!(status.success(), "lumia build failed for {rel}: {status}");

    let mut cmd = Command::new(&exe);
    for (k, v) in run_env {
        cmd.env(k, v);
    }
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
    assert_stdout_lines(rel, &got, expected_lines);
}

pub(crate) fn run_example_env(rel: &str, run_env: &[(&str, &str)], expected_lines: &[&str]) {
    run_example_build_env(rel, None, expected_lines, false, &[], run_env);
}

/// Exact match for non-floats; f64 lines allow tiny platform Display differences
/// (MSVC vs glibc often disagree on the last decimal digit).
fn assert_stdout_lines(rel: &str, got: &[&str], want: &[&str]) {
    if got.len() != want.len() {
        panic!("{rel}: stdout mismatch\n got: {got:?}\n want: {want:?}");
    }
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        if g == w {
            continue;
        }
        match (g.parse::<f64>(), w.parse::<f64>()) {
            (Ok(a), Ok(b)) if float_lines_close(a, b) => continue,
            _ => panic!("{rel}: stdout mismatch at line {i}\n got: {got:?}\n want: {want:?}"),
        }
    }
}

fn float_lines_close(a: f64, b: f64) -> bool {
    if a == b || (a.is_nan() && b.is_nan()) {
        return true;
    }
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() <= scale * 1e-12
}

/// Run `lumia check` and assert failure + required diagnostic substrings.
pub(crate) fn run_check(rel: &str, must_fail: bool, contains: &[&str], contains_any: &[&str]) {
    let root = workspace_root();
    let src = root.join(rel);
    assert!(src.is_file(), "missing example {}", src.display());
    let out = Command::new(lumia_bin())
        .current_dir(&root)
        .args(["check", src.to_str().unwrap()])
        .output()
        .expect("spawn lumia check");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    if must_fail {
        assert!(
            !out.status.success(),
            "{rel} should fail check; output: {combined}"
        );
    } else {
        assert!(
            out.status.success(),
            "{rel} should pass check; output: {combined}"
        );
    }
    for needle in contains {
        assert!(
            combined.contains(needle),
            "{rel}: expected diagnostic containing {needle:?}, got: {combined}"
        );
    }
    if !contains_any.is_empty() {
        assert!(
            contains_any.iter().any(|n| combined.contains(n)),
            "{rel}: expected one of {contains_any:?}, got: {combined}"
        );
    }
}

macro_rules! e2e_ok {
    ($name:ident, $path:expr, $($line:expr),+ $(,)?) => {
        #[test]
        fn $name() {
            crate::harness::run_example($path, &[$($line),+]);
        }
    };
}

macro_rules! e2e_ok_release {
    ($name:ident, $path:expr, $($line:expr),+ $(,)?) => {
        #[test]
        fn $name() {
            crate::harness::run_example_release($path, &[$($line),+]);
        }
    };
}

macro_rules! e2e_reject {
    ($name:ident, $path:expr, $($needle:expr),+ $(,)?) => {
        #[test]
        fn $name() {
            crate::harness::run_check($path, true, &[$($needle),+], &[]);
        }
    };
}

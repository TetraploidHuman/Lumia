// CLI smoke for Phase G/H flags (stock semantics).

#[test]
fn build_no_memo_dense_memo_tf() {
    run_example_build(
        "examples/memo_tf.lm",
        None,
        &["2646700", "2646700", "285"],
        true,
        &["--no-memo-dense"],
    );
}

#[test]
fn build_mm_arc_smoke() {
    run_example_build(
        "examples/poly_map_id.lm",
        None,
        &["2", "true", "2", "true"],
        false,
        &["--mm", "arc"],
    );
}

#[test]
fn build_mm_arc_string_alias() {
    run_example_build(
        "examples/arc_string_alias.lm",
        None,
        &["hello", "world"],
        false,
        &["--mm", "arc"],
    );
}

#[test]
fn build_show_gc_stats_prints_stderr() {
    let root = workspace_root();
    let src = root.join("examples/gc_roots.lm");
    let out = e2e_exe("gc_roots_stats");
    let status = Command::new(lumi_bin())
        .current_dir(&root)
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--show-gc-stats",
        ])
        .status()
        .expect("spawn lumi build");
    assert!(status.success(), "build failed");
    let output = Command::new(&out)
        .output()
        .unwrap_or_else(|e| panic!("run: {e}"));
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("lumi gc:"),
        "expected gc stats on stderr, got: {stderr}"
    );
}

#[test]
fn build_show_memo_stats_prints_stderr() {
    let root = workspace_root();
    let src = root.join("examples/memo_tf.lm");
    let out = e2e_exe("memo_tf_stats");
    let status = Command::new(lumi_bin())
        .current_dir(&root)
        .args([
            "build",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--release",
            "--show-memo-stats",
        ])
        .status()
        .expect("spawn lumi build");
    assert!(status.success(), "build failed");
    let output = Command::new(&out)
        .output()
        .unwrap_or_else(|e| panic!("run: {e}"));
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("lumi memo:"),
        "expected memo stats on stderr, got: {stderr}"
    );
}

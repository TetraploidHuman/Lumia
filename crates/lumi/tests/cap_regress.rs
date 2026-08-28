//! Capability-off regression: disabling Phase C opts must not change semantics.
//!
//! Compares stdout from stock [`CapabilitySet`] vs a capped variant via
//! [`lumi::compile_with_profile`]. Run: `cargo test -p lumi --test cap_regress`
#![cfg(feature = "codegen")]

use lumi::build::compile_with_profile;
use lumi::caps::CapabilitySet;
use lumi::profile::CompileProfile;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    lumi_abi::workspace_root_canonical(env!("CARGO_MANIFEST_DIR"))
}

fn out_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumi_cap_reg_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn exe_path(stem: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    };
    out_dir().join(name)
}

fn run_program(exe: &Path) -> Vec<String> {
    let output = Command::new(exe)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", exe.display()));
    assert!(
        output.status.success(),
        "{} exited {}: stderr={}",
        exe.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn build_and_run(rel: &str, caps: &CapabilitySet, release: bool, stem: &str) -> Vec<String> {
    let root = workspace_root();
    let src = root.join(rel);
    assert!(src.is_file(), "missing {}", src.display());
    let exe = exe_path(stem);
    let profile = CompileProfile::stock(release).with_caps(caps.clone());
    compile_with_profile(&src, &exe, &profile)
        .unwrap_or_else(|e| panic!("compile {rel} caps={caps:?}: {e}"));
    run_program(&exe)
}

fn assert_caps_match_stock(rel: &str, caps: &CapabilitySet, release: bool, tag: &str) {
    let stock = build_and_run(rel, &CapabilitySet::stock(), release, &format!("{tag}_stock"));
    let got = build_and_run(rel, caps, release, &format!("{tag}_caps"));
    assert_eq!(
        stock, got,
        "{rel}: capability-off output diverged from stock\n stock: {stock:?}\n caps:  {got:?}"
    );
}

#[test]
fn hof_fuse_off_matches_stock() {
    assert_caps_match_stock(
        "examples/fuse_hof.lm",
        &CapabilitySet::stock().with_hof_fuse(false),
        false,
        "hof_fuse",
    );
}

#[test]
fn loop_sr_off_matches_stock() {
    assert_caps_match_stock(
        "examples/opt_sr_correctness.lm",
        &CapabilitySet::stock().with_loop_sr(false),
        false,
        "loop_sr",
    );
}

#[test]
fn nsw_iv_off_matches_stock() {
    assert_caps_match_stock(
        "examples/opt_sr_correctness.lm",
        &CapabilitySet::stock().with_nsw_iv(false),
        false,
        "nsw_iv",
    );
}

#[test]
fn tco_off_matches_stock() {
    assert_caps_match_stock(
        "examples/tco_sum_small.lm",
        &CapabilitySet::stock().with_tco(false),
        false,
        "tco",
    );
}

#[test]
fn auto_parallel_off_matches_stock() {
    assert_caps_match_stock(
        "examples/par_map.lm",
        &CapabilitySet::stock().with_auto_parallel(false),
        false,
        "auto_par",
    );
}

#[test]
fn all_codegen_caps_off_matches_stock() {
    let caps = CapabilitySet::stock()
        .with_loop_sr(false)
        .with_tco(false)
        .with_nsw_iv(false);
    assert_caps_match_stock("examples/poly_map_id.lm", &caps, false, "cg_all_off");
}

#[test]
fn hof_fuse_and_codegen_caps_off_matches_stock() {
    let caps = CapabilitySet::stock()
        .with_hof_fuse(false)
        .with_loop_sr(false)
        .with_nsw_iv(false);
    assert_caps_match_stock("examples/fuse_hof.lm", &caps, false, "hof_cg_off");
}

#[test]
fn hof_fuse_off_matches_stock_release() {
    assert_caps_match_stock(
        "examples/fuse_hof.lm",
        &CapabilitySet::stock().with_hof_fuse(false),
        true,
        "hof_fuse_rel",
    );
}

#[test]
fn loop_sr_off_matches_stock_release() {
    assert_caps_match_stock(
        "examples/opt_sr_correctness.lm",
        &CapabilitySet::stock().with_loop_sr(false),
        true,
        "loop_sr_rel",
    );
}

#[test]
fn tco_off_matches_stock_release() {
    assert_caps_match_stock(
        "examples/tco_sum_small.lm",
        &CapabilitySet::stock().with_tco(false),
        true,
        "tco_rel",
    );
}

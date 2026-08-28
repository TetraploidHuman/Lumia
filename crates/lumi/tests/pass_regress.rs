//! Pass-off regression: disabling CoreOpt passes must not change semantics.
#![cfg(feature = "codegen")]

use lumi::build::compile_with_profile;
use lumi::compiler_config::PassDisables;
use lumi::profile::CompileProfile;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    lumi_abi::workspace_root_canonical(env!("CARGO_MANIFEST_DIR"))
}

fn out_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumi_pass_reg_{}", std::process::id()));
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

fn build_and_run(rel: &str, profile: &CompileProfile, stem: &str) -> Vec<String> {
    let root = workspace_root();
    let src = root.join(rel);
    assert!(src.is_file(), "missing {}", src.display());
    let exe = exe_path(stem);
    compile_with_profile(&src, &exe, profile)
        .unwrap_or_else(|e| panic!("compile {rel}: {e}"));
    run_program(&exe)
}

fn assert_pass_off_matches_stock(rel: &str, disables: PassDisables, tag: &str, release: bool) {
    let stock = CompileProfile::stock(release);
    let capped = stock
        .clone()
        .apply_pass_disables(&disables)
        .unwrap_or_else(|e| panic!("apply_pass_disables {tag}: {e}"));
    let stock_out = build_and_run(rel, &stock, &format!("{tag}_stock"));
    let capped_out = build_and_run(rel, &capped, &format!("{tag}_caps"));
    assert_eq!(
        stock_out, capped_out,
        "{rel}: pass-off output diverged from stock\n stock: {stock_out:?}\n caps:  {capped_out:?}"
    );
}

#[test]
fn inline_off_matches_stock_debug() {
    assert_pass_off_matches_stock(
        "examples/poly_map_id.lm",
        PassDisables {
            no_inline: true,
            ..Default::default()
        },
        "inline_dbg",
        false,
    );
}

#[test]
fn inline_off_matches_stock_release() {
    assert_pass_off_matches_stock(
        "examples/poly_map_id.lm",
        PassDisables {
            no_inline: true,
            ..Default::default()
        },
        "inline_rel",
        true,
    );
}

#[test]
fn dense_f64_off_matches_stock() {
    assert_pass_off_matches_stock(
        "examples/float_map_overlay_keys.lm",
        PassDisables {
            no_dense_f64: true,
            ..Default::default()
        },
        "dense_f64",
        false,
    );
}

#[test]
fn repr_select_off_matches_stock() {
    assert_pass_off_matches_stock(
        "examples/small_list_local.lm",
        PassDisables {
            no_repr_select: true,
            ..Default::default()
        },
        "repr_sel",
        false,
    );
}

#[test]
fn escape_off_matches_stock() {
    assert_pass_off_matches_stock(
        "examples/small_map_local.lm",
        PassDisables {
            no_escape: true,
            ..Default::default()
        },
        "escape",
        false,
    );
}

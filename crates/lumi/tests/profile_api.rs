//! CompileProfile API smoke tests.
#![cfg(feature = "codegen")]

use lumi::profile::CompileProfile;
use lumi_opt::{optimize_with, OptProfile, PassSet};

#[test]
fn stock_profile_matches_optimize_stock() {
    let p = CompileProfile::stock(true);
    assert!(p.validate_passes().is_ok());
    assert!(p.pass_set().is_stock(OptProfile::Release));
    assert!(p.pass_names().iter().any(|n| *n == "cse"));
}

#[test]
fn custom_pass_set_without_inline() {
    let p = CompileProfile::stock(true).without_pass("inline");
    assert!(!p.pass_set().contains("inline"));
    let src = r#"
module M
import lumi.io.{println}
val main = { println(1) }
"#;
    let mut core = lumi_opt::compile_source_to_optimized(src, &lumi_opt::OptOptions::for_build(true))
        .expect("frontend");
    // Re-run optimize with custom set (inline off).
    optimize_with(
        &mut core,
        OptProfile::Release,
        p.pass_set(),
        true,
    )
    .expect("optimize_with");
}

#[test]
fn list_caps_and_passes_non_empty() {
    let p = CompileProfile::stock(false);
    let caps = p.format_list_caps();
    assert!(caps.contains("hof_fuse"));
    let passes = p.format_list_passes();
    assert!(passes.contains("cse"));
}

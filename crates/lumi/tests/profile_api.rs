//! CompileProfile API smoke tests.
#![cfg(feature = "codegen")]

use lumi::caps::CapabilitySet;
use lumi::compiler_config::{CapDisables, CompilerConfig, PassDisables};
use lumi::profile::CompileProfile;
use lumi_opt::{optimize_with, OptProfile};

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
    optimize_with(
        &mut core,
        OptProfile::Release,
        p.pass_set(),
        true,
    )
    .expect("optimize_with");
}

#[test]
fn to_pipeline_options_reflects_caps() {
    let p = CompileProfile::stock(false).with_caps(
        CapabilitySet::stock()
            .with_hof_fuse(false)
            .with_auto_parallel(false),
    );
    let pipe = p.to_pipeline_options();
    assert!(!pipe.lower.hof_fuse);
    assert!(!pipe.typecheck.auto_parallel);
}

#[cfg(feature = "codegen")]
#[test]
fn enabled_pass_ids_lists_stock() {
    let p = CompileProfile::stock(true);
    let ids = p.enabled_pass_ids();
    assert!(ids.iter().any(|id| *id == "cse"));
    assert!(!ids.iter().any(|id| *id == "inline") || p.pass_set().contains("inline"));
}

#[cfg(feature = "codegen")]
#[test]
fn assemble_applies_pass_disables() {
    let p = CompileProfile::assemble(
        false,
        true,
        false,
        false,
        vec![],
        &CompilerConfig::default(),
        &CapDisables::default(),
        &PassDisables {
            no_inline: true,
            ..Default::default()
        },
    )
    .expect("assemble");
    assert!(!p.pass_set().contains("inline"));
}

#[test]
fn list_caps_and_passes_non_empty() {
    let p = CompileProfile::stock(false);
    let caps = p.format_list_caps();
    assert!(caps.contains("hof_fuse"));
    let passes = p.format_list_passes();
    assert!(passes.contains("cse"));
}

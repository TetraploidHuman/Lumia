//! Optimization pass pipeline (§7.1 / §7.1.1).
//!
//! Transparent result reuse lives in [`memo`] (DESIGN §7.5):
//! local CSE/fold/LICM + runtime `T_f` (`memo_tf`).
//! Escape analysis + small pure inlining live in [`escape`] / [`inline`].
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::collapsible_match)] // nested Op/Value in escape/memo/inline

mod builtin_effect;
mod concat_ident;
mod copy_elim;
mod dce;
#[cfg(feature = "dense-f64-sr")]
mod dense_f64_sr;
#[cfg(feature = "domain-sr")]
mod domain_sr;
mod escape;
mod inline;
mod ir_util;
mod memo;
#[cfg(feature = "nsw-iv")]
mod nsw_iv;
mod repr_select;
mod specialize_const;

pub(crate) use concat_ident::ConcatIdentPass;
pub(crate) use escape::EscapePass;
pub(crate) use inline::InlinePass;
pub(crate) use memo::{apply_memo_plan, plan_memo_tf, ConstFoldPass, LicmPass};
pub(crate) use specialize_const::SpecializeConstPass;

use copy_elim::CopyElimPass;
use dce::DcePass;
#[cfg(feature = "dense-f64-sr")]
use dense_f64_sr::DenseF64SrPass;
#[cfg(feature = "domain-sr")]
use domain_sr::{CollatzStepsPass, DomainSrPass, FloatOrbitPass, TrialDivOddPass};
use lumia_core::{resolve_module_call_fun_ids, CoreModule, MapRepr};
use memo::cse_module;
#[cfg(feature = "nsw-iv")]
use nsw_iv::NswIvPass;
use repr_select::ReprSelect;

pub struct OptOptions {
    pub release: bool,
    /// Transparent Memo `T_f` (DESIGN §7.5). Defaults to `release`.
    pub memo_tf: bool,
    /// Rewrite dense `List[Float]` nests to `lumia_f64_*` (default on).
    pub dense_f64_sr: bool,
    /// Whole-fn Collatz total/strided + countPrimes → RT Call (Release).
    /// Trial-div odd-step still runs in Debug when the `domain-sr` feature is on.
    pub domain_sr: bool,
}

impl Default for OptOptions {
    fn default() -> Self {
        Self {
            release: false,
            memo_tf: false,
            // Dense float SR is Release / explicit CLI; Debug stays scalar SSA.
            dense_f64_sr: false,
            domain_sr: false,
        }
    }
}

impl OptOptions {
    /// Options for a native build. Memo and dense float SR follow `release`
    /// (Debug stays scalar SSA unless the CLI overrides).
    pub fn for_build(release: bool) -> Self {
        Self {
            release,
            memo_tf: release,
            dense_f64_sr: release && cfg!(feature = "dense-f64-sr"),
            domain_sr: release && cfg!(feature = "domain-sr"),
        }
    }
}

/// Fixed pipeline stages — no `Box<dyn Pass>` allocation on the hot path.
#[derive(Clone, Copy)]
enum PipelinePass {
    Cse,
    ConstFold,
    SpecializeConst,
    Licm,
    Escape,
    #[cfg(feature = "dense-f64-sr")]
    DenseF64Sr,
    #[cfg(feature = "domain-sr")]
    DomainSr,
    #[cfg(feature = "domain-sr")]
    TrialDivOdd,
    #[cfg(feature = "domain-sr")]
    CollatzSteps,
    #[cfg(feature = "domain-sr")]
    FloatOrbit,
    Inline,
    ConcatIdent,
    ReprSelect,
    CopyElim,
    Dce,
    #[cfg(feature = "nsw-iv")]
    NswIv,
}

impl PipelinePass {
    fn name(self) -> &'static str {
        match self {
            Self::Cse => "cse",
            Self::ConstFold => "const_fold",
            Self::SpecializeConst => "specialize_const",
            Self::Licm => "licm",
            Self::Escape => "escape",
            #[cfg(feature = "dense-f64-sr")]
            Self::DenseF64Sr => "dense_f64_sr",
            #[cfg(feature = "domain-sr")]
            Self::DomainSr => "domain_sr",
            #[cfg(feature = "domain-sr")]
            Self::TrialDivOdd => "trial_div_odd",
            #[cfg(feature = "domain-sr")]
            Self::CollatzSteps => "collatz_steps",
            #[cfg(feature = "domain-sr")]
            Self::FloatOrbit => "float_orbit",
            Self::Inline => "inline",
            Self::ConcatIdent => "concat_ident",
            Self::ReprSelect => "repr_select",
            Self::CopyElim => "copy_elim",
            Self::Dce => "dce",
            #[cfg(feature = "nsw-iv")]
            Self::NswIv => "nsw_iv",
        }
    }

    fn run(self, module: &mut CoreModule) {
        match self {
            Self::Cse => CsePass.run(module),
            Self::ConstFold => ConstFoldPass.run(module),
            Self::SpecializeConst => SpecializeConstPass.run(module),
            Self::Licm => LicmPass.run(module),
            Self::Escape => EscapePass.run(module),
            #[cfg(feature = "dense-f64-sr")]
            Self::DenseF64Sr => DenseF64SrPass.run(module),
            #[cfg(feature = "domain-sr")]
            Self::DomainSr => DomainSrPass.run(module),
            #[cfg(feature = "domain-sr")]
            Self::TrialDivOdd => TrialDivOddPass.run(module),
            #[cfg(feature = "domain-sr")]
            Self::CollatzSteps => CollatzStepsPass.run(module),
            #[cfg(feature = "domain-sr")]
            Self::FloatOrbit => FloatOrbitPass.run(module),
            Self::Inline => InlinePass.run(module),
            Self::ConcatIdent => ConcatIdentPass.run(module),
            Self::ReprSelect => ReprSelect.run(module),
            Self::CopyElim => CopyElimPass.run(module),
            Self::Dce => DcePass.run(module),
            #[cfg(feature = "nsw-iv")]
            Self::NswIv => NswIvPass.run(module),
        }
    }
}

struct CsePass;
impl CsePass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        cse_module(module);
    }
}

const DEBUG_PASSES: &[PipelinePass] = &[
    PipelinePass::Cse,
    PipelinePass::ConstFold,
    // Light PE without Inline/memo — bake Int/Bool/Char into leaf clones.
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Licm,
    // Odd-step trial-div in Core so Debug codegen uses the generic loop emit.
    #[cfg(feature = "domain-sr")]
    PipelinePass::TrialDivOdd,
    // collatzSteps → RT (replaces removed codegen cttz SR in Debug builds).
    #[cfg(feature = "domain-sr")]
    PipelinePass::CollatzSteps,
    // floatOrbit → RT (replaces removed codegen `<4|8 x double>` loop IR SR).
    #[cfg(feature = "domain-sr")]
    PipelinePass::FloatOrbit,
    // DenseF64Sr is Release-only (or `dense_f64_sr` via CLI/`for_build`).
    PipelinePass::Escape,
    PipelinePass::ReprSelect,
    PipelinePass::CopyElim,
    PipelinePass::Dce,
    // After SSA settles — codegen reads CoreFun NSW sidecar.
    #[cfg(feature = "nsw-iv")]
    PipelinePass::NswIv,
];
#[cfg(feature = "dense-f64-sr")]
const RELEASE_PASSES: &[PipelinePass] = &[
    PipelinePass::Cse,
    PipelinePass::ConstFold,
    // Rewrite recognizable wholes *before* specialize_const clones fat bodies into
    // `$c_` variants (those clones must then be re-matched; early SR makes clones thin).
    #[cfg(feature = "domain-sr")]
    PipelinePass::DomainSr,
    // Bake Int/Bool/Char call-site constants into leaf clones before inline/PE.
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Licm,
    PipelinePass::DenseF64Sr,
    #[cfg(feature = "domain-sr")]
    PipelinePass::DomainSr,
    PipelinePass::Inline,
    // Inlined nests / composed helpers — second SR before fold/specialize.
    PipelinePass::DenseF64Sr,
    #[cfg(feature = "domain-sr")]
    PipelinePass::DomainSr,
    // After last whole-fn SR: leftover `isPrime` loops (must not run before
    // DomainSr, which matches inlined `d+=1` trial-div).
    #[cfg(feature = "domain-sr")]
    PipelinePass::TrialDivOdd,
    // Inline exposes fresh pure exprs / literals — CSE then fold/specialize.
    PipelinePass::Cse,
    PipelinePass::ConstFold,
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Licm,
    PipelinePass::Escape,
    PipelinePass::ConcatIdent,
    PipelinePass::ConstFold,
    PipelinePass::ReprSelect,
    PipelinePass::CopyElim,
    PipelinePass::Dce,
    #[cfg(feature = "nsw-iv")]
    PipelinePass::NswIv,
];

#[cfg(not(feature = "dense-f64-sr"))]
const RELEASE_PASSES: &[PipelinePass] = &[
    PipelinePass::Cse,
    PipelinePass::ConstFold,
    #[cfg(feature = "domain-sr")]
    PipelinePass::DomainSr,
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Licm,
    #[cfg(feature = "domain-sr")]
    PipelinePass::DomainSr,
    PipelinePass::Inline,
    #[cfg(feature = "domain-sr")]
    PipelinePass::DomainSr,
    #[cfg(feature = "domain-sr")]
    PipelinePass::TrialDivOdd,
    PipelinePass::Cse,
    PipelinePass::ConstFold,
    PipelinePass::SpecializeConst,
    PipelinePass::ConstFold,
    PipelinePass::Licm,
    PipelinePass::Escape,
    PipelinePass::ConcatIdent,
    PipelinePass::ConstFold,
    PipelinePass::ReprSelect,
    PipelinePass::CopyElim,
    PipelinePass::Dce,
    #[cfg(feature = "nsw-iv")]
    PipelinePass::NswIv,
];

/// Frontend → Core → optimize for **Core IR fixtures / unit tests**.
///
/// **Not** a substitute for the CLI/`check_program` pipeline: this path uses
/// [`lumia_core::compile_source_to_core*`] (single-buffer, no loader / `std.*` /
/// package graph). Prefer `lumia::compile_program_to_optimized` (loader/std on)
/// or `optimize(&mut CoreModule, …)` on IR built via the full check path when
/// validating import/std/package behavior.
pub fn compile_source_to_optimized(src: &str, opts: &OptOptions) -> Result<CoreModule, String> {
    compile_source_to_optimized_with_frontend(src, opts, &lumia_core::FrontendOptions::default())
}

/// Same as [`compile_source_to_optimized`] with explicit frontend options.
///
/// Still skips loader/`std`; use `lumia::compile_program_to_optimized` for the
/// real package/std path.
pub fn compile_source_to_optimized_with_frontend(
    src: &str,
    opts: &OptOptions,
    frontend: &lumia_core::FrontendOptions,
) -> Result<CoreModule, String> {
    let mut core = lumia_core::compile_source_to_core_with_options(src, frontend)?;
    optimize(&mut core, opts);
    Ok(core)
}

/// Read a `.lm` file and compile through optimize (**fixture path**; no loader).
pub fn compile_file_to_optimized(
    path: &std::path::Path,
    opts: &OptOptions,
) -> Result<CoreModule, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    compile_source_to_optimized(&src, opts)
}

/// Run the standard pipeline. Uncertain → default stable paths (§7.1.1).
pub fn optimize(module: &mut CoreModule, opts: &OptOptions) {
    // Plan transparent Memo on the pre-CSE module (reuse evidence needs duplicate calls).
    let memo_plan = if opts.memo_tf {
        Some(plan_memo_tf(module))
    } else {
        None
    };

    // Stamp Memo *before* Release inline / specialize so T_f callees are not
    // absorbed into callers (which would drop runtime result reuse).
    if let Some(ref plan) = memo_plan {
        apply_memo_plan(module, plan);
    }

    // Stamp CallTarget.id before any pass that reads direct-call edges; re-stamp
    // after inline/specialize/domain_sr so codegen sees stable FunIds.
    resolve_module_call_fun_ids(module);

    let passes = if opts.release {
        RELEASE_PASSES
    } else {
        DEBUG_PASSES
    };
    for p in passes {
        #[cfg(feature = "dense-f64-sr")]
        if matches!(p, PipelinePass::DenseF64Sr) && !opts.dense_f64_sr {
            continue;
        }
        #[cfg(feature = "domain-sr")]
        if matches!(p, PipelinePass::DomainSr) && !opts.domain_sr {
            continue;
        }
        p.run(module);
    }

    resolve_module_call_fun_ids(module);
}

/// Named passes for tooling / diagnostics — order matches [`optimize`].
///
/// Release lists `"memo_tf"` **first**: planning/apply run via [`plan_memo_tf`]
/// before CSE (not as a `PipelinePass::run`); later inline/specialize see `memo`
/// and leave T_f callees intact. Re-planning after CSE would drop const-reuse
/// evidence (§7.5.2).
pub fn pass_names(release: bool) -> Vec<&'static str> {
    let mut names = Vec::new();
    if release {
        names.push("memo_tf");
    }
    let pipeline: Vec<&'static str> = if release {
        RELEASE_PASSES.iter().map(|p| p.name()).collect()
    } else {
        DEBUG_PASSES.iter().map(|p| p.name()).collect()
    };
    names.extend(pipeline);
    names
}

/// Default Map representation when analysis cannot prove a better choice.
pub fn default_map_repr() -> MapRepr {
    MapRepr::HashOrdered
}

#[cfg(test)]
mod tests {
    use super::*;
    use copy_elim::CopyElimPass;
    use lumia_core::{Block, CoreFun, CoreModule, FunKind, ListRepr, Local, Op, Value};
    use lumia_ty::{Effect, Type};
    use repr_select::ReprSelect;
    use rustc_hash::FxHashSet as HashSet;

    #[test]
    fn defaults() {
        assert_eq!(default_map_repr(), MapRepr::HashOrdered);
    }

    #[test]
    fn compile_source_to_optimized_skips_loader_std() {
        // Fixture path must not pretend to be check_program: import aliases stay unbound
        // (surface `println` is a free builtin; aliased `log` is not).
        let err = compile_source_to_optimized(
            r#"
module Main
import std.io.{println as log}
val main = { log(1) }
"#,
            &OptOptions::default(),
        )
        .expect_err("aliased std import without loader must fail");
        let msg = err.to_lowercase();
        assert!(
            msg.contains("unbound") || msg.contains("log"),
            "expected unbound `log`, got {err}"
        );
    }

    #[test]
    fn for_build_ties_dense_sr_and_memo_to_release() {
        let dbg = OptOptions::for_build(false);
        assert!(!dbg.release && !dbg.memo_tf && !dbg.dense_f64_sr && !dbg.domain_sr);
        let rel = OptOptions::for_build(true);
        assert!(rel.release && rel.memo_tf && rel.dense_f64_sr);
        #[cfg(feature = "domain-sr")]
        assert!(rel.domain_sr);
        #[cfg(not(feature = "domain-sr"))]
        assert!(!rel.domain_sr);
    }

    #[test]
    fn pass_pipeline_names() {
        assert!(pass_names(true).contains(&"inline"));
        assert!(pass_names(true).contains(&"escape"));
        assert!(pass_names(true).contains(&"copy_elim"));
        assert!(pass_names(true).contains(&"dce"));
        assert!(pass_names(true).contains(&"const_fold"));
        assert!(pass_names(true).contains(&"specialize_const"));
        assert!(pass_names(true).contains(&"licm"));
        assert!(pass_names(true).contains(&"concat_ident"));
        assert_eq!(pass_names(true).first(), Some(&"memo_tf"));
        assert!(!pass_names(false).contains(&"inline"));
        assert!(pass_names(false).contains(&"specialize_const"));
        assert!(pass_names(false).contains(&"dce"));
        assert!(!pass_names(false).contains(&"memo_tf"));
        assert!(!pass_names(false).contains(&"dense_f64_sr"));
        #[cfg(feature = "dense-f64-sr")]
        assert!(pass_names(true).contains(&"dense_f64_sr"));
        #[cfg(not(feature = "dense-f64-sr"))]
        assert!(!pass_names(true).contains(&"dense_f64_sr"));
        #[cfg(feature = "domain-sr")]
        {
            assert!(pass_names(true).contains(&"domain_sr"));
            assert!(!pass_names(false).contains(&"domain_sr"));
            assert!(pass_names(true).contains(&"trial_div_odd"));
            assert!(pass_names(false).contains(&"trial_div_odd"));
        }
        #[cfg(not(feature = "domain-sr"))]
        {
            assert!(!pass_names(true).contains(&"domain_sr"));
            assert!(!pass_names(true).contains(&"trial_div_odd"));
        }
        #[cfg(feature = "nsw-iv")]
        {
            assert!(pass_names(true).contains(&"nsw_iv"));
            assert!(pass_names(false).contains(&"nsw_iv"));
        }
        #[cfg(not(feature = "nsw-iv"))]
        {
            assert!(!pass_names(true).contains(&"nsw_iv"));
            assert!(!pass_names(false).contains(&"nsw_iv"));
        }
    }

    #[test]
    fn dual_track_specialization_stages_are_separated() {
        // Type mono (`$Float` / MonoKey) runs in `run_core_abi_pipeline` (post-lower).
        // Const specialize (`$c_`) is opt-only (`specialize_const` in DEBUG/RELEASE).
        // Lock-in: opt pipeline never claims a "mono" pass; specialize_const stays.
        let release = pass_names(true);
        let debug = pass_names(false);
        assert!(
            release.iter().any(|n| *n == "specialize_const")
                && debug.iter().any(|n| *n == "specialize_const"),
            "SpecializeConst must remain in both pipelines"
        );
        assert!(
            !release.iter().any(|n| n.contains("mono"))
                && !debug.iter().any(|n| n.contains("mono")),
            "type mono must not appear as an opt PipelinePass, got release={release:?}"
        );
        // Release runs SpecializeConst twice (pre/post inline); Debug once.
        assert_eq!(
            release.iter().filter(|n| **n == "specialize_const").count(),
            2
        );
        assert_eq!(
            debug.iter().filter(|n| **n == "specialize_const").count(),
            1
        );
    }

    #[test]
    fn pass_pipeline_exact_order() {
        // Debug: CSE → fold → specialize → fold → LICM → [trial_div_odd] →
        // Escape → ReprSelect (no inline/memo/dense_f64_sr).
        let mut debug_expected = vec![
            "cse",
            "const_fold",
            "specialize_const",
            "const_fold",
            "licm",
        ];
        #[cfg(feature = "domain-sr")]
        {
            debug_expected.push("trial_div_odd");
            debug_expected.push("collatz_steps");
            debug_expected.push("float_orbit");
        }
        debug_expected.extend(["escape", "repr_select", "copy_elim", "dce"]);
        #[cfg(feature = "nsw-iv")]
        debug_expected.push("nsw_iv");
        assert_eq!(
            DEBUG_PASSES.iter().map(|p| p.name()).collect::<Vec<_>>(),
            debug_expected
        );
        // Release interleaves specialize/fold/inline; Escape must immediately
        // precede ReprSelect (ConcatIdent/ConstFold in between do not allocate).
        #[cfg(all(feature = "dense-f64-sr", feature = "nsw-iv"))]
        assert_eq!(
            RELEASE_PASSES.iter().map(|p| p.name()).collect::<Vec<_>>(),
            {
                #[cfg(feature = "domain-sr")]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "domain_sr",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "dense_f64_sr",
                        "domain_sr",
                        "inline",
                        "dense_f64_sr",
                        "domain_sr",
                        "trial_div_odd",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                        "nsw_iv",
                    ]
                }
                #[cfg(not(feature = "domain-sr"))]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "dense_f64_sr",
                        "inline",
                        "dense_f64_sr",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                        "nsw_iv",
                    ]
                }
            }
        );
        #[cfg(all(feature = "dense-f64-sr", not(feature = "nsw-iv")))]
        assert_eq!(
            RELEASE_PASSES.iter().map(|p| p.name()).collect::<Vec<_>>(),
            {
                #[cfg(feature = "domain-sr")]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "domain_sr",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "dense_f64_sr",
                        "domain_sr",
                        "inline",
                        "dense_f64_sr",
                        "domain_sr",
                        "trial_div_odd",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                    ]
                }
                #[cfg(not(feature = "domain-sr"))]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "dense_f64_sr",
                        "inline",
                        "dense_f64_sr",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                    ]
                }
            }
        );
        #[cfg(all(not(feature = "dense-f64-sr"), feature = "nsw-iv"))]
        assert_eq!(
            RELEASE_PASSES.iter().map(|p| p.name()).collect::<Vec<_>>(),
            {
                #[cfg(feature = "domain-sr")]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "domain_sr",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "domain_sr",
                        "inline",
                        "domain_sr",
                        "trial_div_odd",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                        "nsw_iv",
                    ]
                }
                #[cfg(not(feature = "domain-sr"))]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "inline",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                        "nsw_iv",
                    ]
                }
            }
        );
        #[cfg(all(not(feature = "dense-f64-sr"), not(feature = "nsw-iv")))]
        assert_eq!(
            RELEASE_PASSES.iter().map(|p| p.name()).collect::<Vec<_>>(),
            {
                #[cfg(feature = "domain-sr")]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "domain_sr",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "domain_sr",
                        "inline",
                        "domain_sr",
                        "trial_div_odd",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                    ]
                }
                #[cfg(not(feature = "domain-sr"))]
                {
                    vec![
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "inline",
                        "cse",
                        "const_fold",
                        "specialize_const",
                        "const_fold",
                        "licm",
                        "escape",
                        "concat_ident",
                        "const_fold",
                        "repr_select",
                        "copy_elim",
                        "dce",
                    ]
                }
            }
        );
        let release = pass_names(true);
        let escape_i = release.iter().position(|&n| n == "escape").unwrap();
        let repr_i = release.iter().position(|&n| n == "repr_select").unwrap();
        assert!(escape_i < repr_i);
        // No second Escape after the Escape→ReprSelect pair today.
        assert_eq!(
            RELEASE_PASSES
                .iter()
                .filter(|p| matches!(p, PipelinePass::Escape))
                .count(),
            1
        );
    }

    #[test]
    fn copy_elim_collapses_alias() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(42),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Local(Local(0)),
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(1)),
                },
                ret_ty: Type::Int,
                effect: Effect::pure(),
                is_main: false,
                memo: None,
                external: None,
                foreign_abi: lumia_core::ForeignAbi::C,
                escaping: HashSet::default(),
                nsw_binop_locals: Default::default(),
                safe_divisor_locals: Default::default(),
                nonneg_iv_load_locals: Default::default(),
                scheme_poly: false,
                mono_of: None,
                kind: FunKind::Normal,
            }],
        );
        CopyElimPass.run(&mut module);
        let f = &module.functions[0];
        assert_eq!(f.body.ops.len(), 1);
        assert_eq!(f.body.result, Some(Local(0)));
    }

    #[test]
    fn repr_select_marks_nonescaping_small_list_lit() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::AllocList {
                                elems: vec![Local(0)],
                                repr: ListRepr::HeapList,
                            },
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::Int(0),
                            pure_region: true,
                        },
                    ],
                    // Return a non-list so the list itself does not escape.
                    result: Some(Local(2)),
                },
                ret_ty: Type::Int,
                effect: Effect::pure(),
                is_main: false,
                memo: None,
                external: None,
                foreign_abi: lumia_core::ForeignAbi::C,
                escaping: HashSet::default(),
                nsw_binop_locals: Default::default(),
                safe_divisor_locals: Default::default(),
                nonneg_iv_load_locals: Default::default(),
                scheme_poly: false,
                mono_of: None,
                kind: FunKind::Normal,
            }],
        );
        EscapePass.run(&mut module);
        ReprSelect.run(&mut module);
        let Op::Let { value, .. } = &module.functions[0].body.ops[1] else {
            panic!("expected let");
        };
        match value {
            Value::AllocList { repr, .. } => assert_eq!(*repr, ListRepr::LitList),
            other => panic!("expected AllocList, got {other:?}"),
        }
    }

    #[test]
    fn repr_select_escaping_small_list_stays_heap() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::AllocList {
                                elems: vec![Local(0)],
                                repr: ListRepr::HeapList,
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(1)),
                },
                ret_ty: Type::List(Box::new(Type::Int)),
                effect: Effect::pure(),
                is_main: false,
                memo: None,
                external: None,
                foreign_abi: lumia_core::ForeignAbi::C,
                escaping: HashSet::default(),
                nsw_binop_locals: Default::default(),
                safe_divisor_locals: Default::default(),
                nonneg_iv_load_locals: Default::default(),
                scheme_poly: false,
                mono_of: None,
                kind: FunKind::Normal,
            }],
        );
        EscapePass.run(&mut module);
        ReprSelect.run(&mut module);
        let Op::Let { value, .. } = &module.functions[0].body.ops[1] else {
            panic!("expected let");
        };
        match value {
            Value::AllocList { repr, .. } => assert_eq!(*repr, ListRepr::HeapList),
            other => panic!("expected AllocList, got {other:?}"),
        }
    }

    #[test]
    fn repr_select_end_to_end_small_listof() {
        use lumia_hir::lower_module;
        use lumia_syntax::parse_module;
        use lumia_ty::infer_module;
        let src = r#"
module M
val main = {
    val xs = listOf(10, 20, 30)
    xs.len()
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        let mut core = lumia_core::lower_hir_with_schemes(
            &typed.module,
            &typed.fun_types,
            &typed.fun_schemes,
            &typed.type_at,
            &[],
        )
        .expect("core");
        lumia_core::run_core_abi_pipeline(&mut core);
        optimize(&mut core, &OptOptions::default());
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let alloc = main.body.ops.iter().find_map(|op| match op {
            Op::Let {
                value: Value::AllocList { elems, repr },
                ..
            } if !elems.is_empty() => Some(*repr),
            _ => None,
        });
        assert_eq!(
            alloc,
            Some(ListRepr::LitList),
            "expected LitList for non-escaping listOf; escaping={:?}",
            main.escaping
        );
    }

    #[test]
    fn repr_select_list_field_of_wide_product_is_heap() {
        use lumia_hir::lower_module;
        use lumia_syntax::parse_module;
        use lumia_ty::infer_module;
        // >8 fields ⇒ HeapAdt; list field must not stay LitList (GC / UAF).
        let src = r#"
module M
type Wide {
    val a0
    val a1
    val a2
    val a3
    val a4
    val a5
    val a6
    val a7
    val a8
    val xs
}
val main = {
    val w = Wide {
        a0 = 0, a1 = 1, a2 = 2, a3 = 3, a4 = 4,
        a5 = 5, a6 = 6, a7 = 7, a8 = 8,
        xs = listOf(10, 20)
    }
    w.a0
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        let mut core = lumia_core::lower_hir_with_schemes(
            &typed.module,
            &typed.fun_types,
            &typed.fun_schemes,
            &typed.type_at,
            &[],
        )
        .expect("core");
        lumia_core::run_core_abi_pipeline(&mut core);
        optimize(&mut core, &OptOptions::default());
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let list_repr = main.body.ops.iter().find_map(|op| match op {
            Op::Let {
                value: Value::AllocList { elems, repr },
                ..
            } if elems.len() == 2 => Some(*repr),
            _ => None,
        });
        assert_eq!(
            list_repr,
            Some(ListRepr::HeapList),
            "list field of wide HeapAdt must be HeapList; escaping={:?}",
            main.escaping
        );
    }

    #[test]
    fn repr_select_empty_list_is_lit() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    ops: vec![Op::Let {
                        local: Local(0),
                        value: Value::AllocList {
                            elems: vec![],
                            repr: ListRepr::HeapList,
                        },
                        pure_region: true,
                    }],
                    result: Some(Local(0)),
                },
                ret_ty: Type::List(Box::new(Type::Int)),
                effect: Effect::pure(),
                is_main: false,
                memo: None,
                external: None,
                foreign_abi: lumia_core::ForeignAbi::C,
                escaping: HashSet::default(),
                nsw_binop_locals: Default::default(),
                safe_divisor_locals: Default::default(),
                nonneg_iv_load_locals: Default::default(),
                scheme_poly: false,
                mono_of: None,
                kind: FunKind::Normal,
            }],
        );
        EscapePass.run(&mut module);
        ReprSelect.run(&mut module);
        let Op::Let { value, .. } = &module.functions[0].body.ops[0] else {
            panic!("expected let");
        };
        match value {
            Value::AllocList { repr, .. } => assert_eq!(*repr, ListRepr::LitList),
            other => panic!("expected AllocList, got {other:?}"),
        }
    }
}

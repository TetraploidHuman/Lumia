//! Optimization pass pipeline (§7.1 / §7.1.1).
//!
//! Transparent result reuse lives in the `memo` module (DESIGN §7.5):
//! local CSE/fold/LICM + runtime `T_f` (`memo_tf`).
//! Escape analysis + small pure inlining live behind `opt-repr-stack` / `opt-inline`.
//!
//! Pass inventory and schedules: [`ALL_PASSES`] / [`PassSet`] / [`OptProfile`].

mod copy_elim;
#[cfg(feature = "opt-dense-f64")]
mod dense_f64_sr;
#[cfg(feature = "opt-repr-stack")]
mod escape;
mod fusion;
#[cfg(feature = "opt-inline")]
mod inline;
mod ir_util;
mod memo;
mod pipeline;
mod registry;
#[cfg(feature = "opt-repr-stack")]
mod repr_select;
mod specialize_const;
#[cfg(feature = "opt-repr-stack")]
mod use_summary;

#[cfg(feature = "opt-repr-stack")]
pub use escape::{escaping_locals, EscapePass};
pub use fusion::ConcatIdentPass;
#[cfg(feature = "opt-inline")]
pub use inline::InlinePass;
pub use memo::{
    apply_memo_plan, plan_memo_tf, ConstFoldPass, LicmPass, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_SLOTS_TABLE_BYTES, MEMO_TF_MAX_ARGS,
    MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
};
pub use pipeline::{
    pass_names_for, validate_pass_set, IrAnno, OptProfile, PassInfo, PassKind, PassSet, PassStage,
};
pub use specialize_const::SpecializeConstPass;

pub use registry::{info as pass_info, ALL as ALL_PASSES};

use lumi_core::{CoreModule, ListRepr, MapRepr};
use pipeline::{build_schedule, run_schedule, schedule_for};

#[derive(Debug, Clone)]
pub struct OptOptions {
    pub release: bool,
    /// Transparent Memo `T_f` (DESIGN §7.5). Defaults to `release`.
    pub memo_tf: bool,
    /// Prefer DenseInt `T_f` tables over slot tables when eligible (§7.5.3).
    pub memo_prefer_dense: bool,
}

impl Default for OptOptions {
    fn default() -> Self {
        Self {
            release: false,
            memo_tf: false,
            memo_prefer_dense: true,
        }
    }
}

impl OptOptions {
    pub fn for_build(release: bool) -> Self {
        Self {
            release,
            memo_tf: release,
            memo_prefer_dense: true,
        }
    }

    pub fn profile(&self) -> OptProfile {
        OptProfile::from_release(self.release)
    }
}

pub trait Pass {
    fn name(&self) -> &str;
    fn run(&self, module: &mut CoreModule);
}

/// Frontend → Core → optimize (for tests and tooling).
pub fn compile_source_to_optimized(src: &str, opts: &OptOptions) -> Result<CoreModule, String> {
    compile_source_to_optimized_with_pipeline(src, opts, &lumi_core::PipelineOptions::default())
}

/// Same as [`compile_source_to_optimized`] with explicit frontend pipeline options.
pub fn compile_source_to_optimized_with_pipeline(
    src: &str,
    opts: &OptOptions,
    frontend: &lumi_core::PipelineOptions,
) -> Result<CoreModule, String> {
    let mut core = lumi_core::compile_source_to_core_with_pipeline(src, frontend)?;
    optimize(&mut core, opts);
    Ok(core)
}

/// Same as [`compile_source_to_optimized`] with legacy typecheck-only frontend.
pub fn compile_source_to_optimized_with_frontend(
    src: &str,
    opts: &OptOptions,
    frontend: &lumi_core::FrontendOptions,
) -> Result<CoreModule, String> {
    compile_source_to_optimized_with_pipeline(
        src,
        opts,
        &lumi_core::PipelineOptions {
            lower: lumi_hir::LowerOptions::default(),
            typecheck: frontend.clone(),
        },
    )
}

/// Read a `.lm` file and compile through optimize.
pub fn compile_file_to_optimized(
    path: &std::path::Path,
    opts: &OptOptions,
) -> Result<CoreModule, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    compile_source_to_optimized(&src, opts)
}

/// Run the standard Debug/Release pipeline. Uncertain → default stable paths (§7.1.1).
pub fn optimize(module: &mut CoreModule, opts: &OptOptions) {
    let profile = opts.profile();
    let set = PassSet::for_profile(profile);
    // Stock path: static schedule slice (no allocation).
    optimize_with(module, profile, &set, opts.memo_tf, opts.memo_prefer_dense)
        .expect("stock PassSet must validate");
}

/// Assemble from [`OptProfile`] + [`PassSet`].
///
/// Memo planning runs when `memo_tf` is true and either the set enables `memo_tf`
/// or this is a stock profile (Debug omits the id from the set for naming, but
/// still honors [`OptOptions::memo_tf`] for tooling compatibility).
/// Filtered sets allocate a schedule `Vec`; stock sets use a static slice.
pub fn optimize_with(
    module: &mut CoreModule,
    profile: OptProfile,
    set: &PassSet,
    memo_tf: bool,
    memo_prefer_dense: bool,
) -> Result<(), String> {
    validate_pass_set(set)?;

    let do_memo = memo_tf && (set.contains("memo_tf") || set.is_stock(profile));
    // Plan transparent Memo on the pre-CSE module (reuse evidence needs duplicate calls).
    let memo_plan = if do_memo {
        Some(plan_memo_tf(module, memo_prefer_dense))
    } else {
        None
    };

    // Stamp Memo *before* Release inline / specialize so T_f callees are not
    // absorbed into callers (which would drop runtime result reuse).
    if let Some(ref plan) = memo_plan {
        apply_memo_plan(module, plan);
    }

    if set.is_stock(profile) {
        run_schedule(module, schedule_for(profile));
    } else {
        let passes = build_schedule(profile, set)?;
        run_schedule(module, &passes);
    }
    Ok(())
}

/// Named passes for tooling / diagnostics.
///
/// `"memo_tf"` is listed for Release even though planning runs via [`plan_memo_tf`]
/// *before* CSE (not as a `Pass::run`); the plan is applied immediately so later
/// inline/specialize see `memo` and leave T_f callees intact. Re-planning after
/// CSE would drop const-reuse evidence (§7.5.2).
pub fn pass_names(release: bool) -> Vec<&'static str> {
    let profile = OptProfile::from_release(release);
    pass_names_for(profile, &PassSet::for_profile(profile))
}

/// Default Map representation when analysis cannot prove a better choice.
pub fn default_map_repr() -> MapRepr {
    MapRepr::HashOrdered
}

/// Default List representation when analysis cannot prove a better choice.
pub fn default_list_repr() -> ListRepr {
    ListRepr::HeapList
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "opt-repr-stack")]
    use super::EscapePass;
    use super::*;
    use crate::copy_elim::CopyElimPass;
    use crate::pipeline::{PipelinePass, DEBUG_SCHEDULE, RELEASE_SCHEDULE};
    #[cfg(feature = "opt-repr-stack")]
    use crate::repr_select::ReprSelect;
    use lumi_core::{Block, CoreFun, Local, Op, Value};
    use lumi_ty::{Effect, Type};
    use rustc_hash::FxHashSet as HashSet;

    #[test]
    fn println_mono_survives_debug_optimize() {
        let mut core = lumi_core::compile_source_to_core(
            r#"
module HofFloatApply
val println(x) = { __println(x) }
val dbl(x) = x + x
val apply(f, x) = f(x)
val main = {
    println(dbl(1.5))
    println(apply(dbl, 1.5))
    println(apply(dbl, 2.0))
}
"#,
        )
        .expect("core");
        optimize(
            &mut core,
            &OptOptions {
                release: false,
                memo_tf: false,
                memo_prefer_dense: true,
            },
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let println_calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun.starts_with("println") => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            println_calls
                .iter()
                .filter(|c| c.contains("$Float"))
                .count(),
            3,
            "optimize must keep println$Float, got {println_calls:?}"
        );
    }

    #[test]
    fn defaults() {
        assert_eq!(default_list_repr(), ListRepr::HeapList);
        assert_eq!(default_map_repr(), MapRepr::HashOrdered);
    }

    #[test]
    fn pass_pipeline_names() {
        assert!(pass_names(true).contains(&"copy_elim"));
        assert!(pass_names(true).contains(&"const_fold"));
        assert!(pass_names(true).contains(&"specialize_const"));
        assert!(pass_names(true).contains(&"licm"));
        assert!(pass_names(true).contains(&"concat_ident"));
        assert!(pass_names(false).contains(&"specialize_const"));
        #[cfg(feature = "opt-inline")]
        {
            assert!(pass_names(true).contains(&"inline"));
            assert!(!pass_names(false).contains(&"inline"));
        }
        #[cfg(feature = "opt-repr-stack")]
        assert!(pass_names(true).contains(&"escape"));
        #[cfg(feature = "opt-memo")]
        {
            assert!(pass_names(true).contains(&"memo_tf"));
            assert!(!pass_names(false).contains(&"memo_tf"));
        }
        #[cfg(not(feature = "opt-memo"))]
        assert!(!pass_names(true).contains(&"memo_tf"));
    }

    #[test]
    #[cfg(all(
        feature = "opt-dense-f64",
        feature = "opt-inline",
        feature = "opt-repr-stack"
    ))]
    fn pass_pipeline_exact_order() {
        // Debug: CSE → fold → specialize → fold → LICM → dense_f64_sr → Escape → ReprSelect
        // (no inline/memo).
        assert_eq!(
            DEBUG_SCHEDULE.iter().map(|p| p.name()).collect::<Vec<_>>(),
            vec![
                "cse",
                "const_fold",
                "specialize_const",
                "const_fold",
                "licm",
                "dense_f64_sr",
                "escape",
                "repr_select"
            ]
        );
        // Release interleaves specialize/fold/inline; Escape must immediately
        // precede ReprSelect (ConcatIdent/ConstFold in between do not allocate).
        assert_eq!(
            RELEASE_SCHEDULE
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>(),
            vec![
                "cse",
                "const_fold",
                "specialize_const",
                "const_fold",
                "licm",
                "dense_f64_sr",
                "inline",
                "dense_f64_sr",
                "const_fold",
                "specialize_const",
                "const_fold",
                "escape",
                "concat_ident",
                "const_fold",
                "repr_select",
                "copy_elim",
            ]
        );
        let release = pass_names(true);
        let escape_i = release.iter().position(|&n| n == "escape").unwrap();
        let repr_i = release.iter().position(|&n| n == "repr_select").unwrap();
        assert!(escape_i < repr_i);
        assert_eq!(
            RELEASE_SCHEDULE
                .iter()
                .filter(|p| matches!(p, PipelinePass::Escape))
                .count(),
            1
        );
    }

    #[test]
    #[cfg(all(
        feature = "opt-inline",
        feature = "opt-repr-stack",
        feature = "opt-memo"
    ))]
    fn optimize_with_filtered_drops_inline() {
        let set = PassSet::for_profile(OptProfile::Release).without("inline");
        validate_pass_set(&set).unwrap();
        let names = pass_names_for(OptProfile::Release, &set);
        assert!(!names.contains(&"inline"));
        assert!(names.contains(&"escape"));
        assert!(names.contains(&"memo_tf"));
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
                    params: vec![],
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
                escaping: HashSet::default(),
                scheme_poly: false,
                mono_of: None,
            }],
        );
        CopyElimPass.run(&mut module);
        let f = &module.functions[0];
        assert_eq!(f.body.ops.len(), 1);
        assert_eq!(f.body.result, Some(Local(0)));
    }

    #[test]
    #[cfg(feature = "opt-repr-stack")]
    fn repr_select_marks_nonescaping_small_list_lit() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    params: vec![],
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
                escaping: HashSet::default(),
                scheme_poly: false,
                mono_of: None,
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
    #[cfg(feature = "opt-repr-stack")]
    fn repr_select_escaping_small_list_stays_heap() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    params: vec![],
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
                escaping: HashSet::default(),
                scheme_poly: false,
                mono_of: None,
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
    #[cfg(feature = "opt-repr-stack")]
    fn repr_select_end_to_end_small_listof() {
        use lumi_hir::lower_module;
        use lumi_syntax::parse_module;
        use lumi_ty::infer_module;
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
        let mut core =
            lumi_core::lower_hir_with_schemes(&typed.module, &typed.fun_types, &typed.fun_schemes);
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
    #[cfg(feature = "opt-repr-stack")]
    fn repr_select_list_field_of_wide_product_is_heap() {
        use lumi_hir::lower_module;
        use lumi_syntax::parse_module;
        use lumi_ty::infer_module;
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
        let mut core =
            lumi_core::lower_hir_with_schemes(&typed.module, &typed.fun_types, &typed.fun_schemes);
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
    #[cfg(feature = "opt-repr-stack")]
    fn repr_select_empty_list_is_lit() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    params: vec![],
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
                escaping: HashSet::default(),
                scheme_poly: false,
                mono_of: None,
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

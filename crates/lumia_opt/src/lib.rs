//! Optimization pass pipeline (§7.1 / §7.1.1).
//!
//! Transparent result reuse lives in [`memo`] (DESIGN §7.5):
//! local CSE/fold/LICM + runtime `T_f` (`memo_tf`).
//! Escape analysis + small pure inlining live in [`escape`] / [`inline`].

mod copy_elim;
mod escape;
mod fusion;
mod inline;
mod memo;
mod repr_select;

pub use escape::{escaping_locals, EscapePass};
pub use fusion::ConcatIdentPass;
pub use inline::InlinePass;
pub use memo::{
    apply_memo_plan, plan_memo_tf, ConstFoldPass, LicmPass, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_L2_MAX_ARGS, MEMO_L2_MAX_FUNS, MEMO_L2_SLOTS, MEMO_PROCESS_BYTE_CAP,
    MEMO_SLOTS_TABLE_BYTES, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
};

use copy_elim::CopyElimPass;
use lumia_core::{CoreModule, ListRepr, MapRepr};
use memo::cse_module;
use repr_select::ReprSelect;

#[derive(Default)]
pub struct OptOptions {
    pub release: bool,
    /// Transparent Memo `T_f` (DESIGN §7.5). Defaults to `release`.
    pub memo_tf: bool,
}

impl OptOptions {
    pub fn for_build(release: bool) -> Self {
        Self {
            release,
            memo_tf: release,
        }
    }
}

pub trait Pass {
    fn name(&self) -> &str;
    fn run(&self, module: &mut CoreModule);
}

/// Frontend → Core → optimize (for tests and tooling).
pub fn compile_source_to_optimized(src: &str, opts: &OptOptions) -> Result<CoreModule, String> {
    compile_source_to_optimized_with_frontend(src, opts, &lumia_core::FrontendOptions::default())
}

/// Same as [`compile_source_to_optimized`] with explicit frontend options.
pub fn compile_source_to_optimized_with_frontend(
    src: &str,
    opts: &OptOptions,
    frontend: &lumia_core::FrontendOptions,
) -> Result<CoreModule, String> {
    let mut core = lumia_core::compile_source_to_core_with_options(src, frontend)?;
    optimize(&mut core, opts);
    Ok(core)
}

/// Read a `.lm` file and compile through optimize.
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

    // Local reuse always: CSE + const-fold/copy-prop + LICM (semantic-preserving).
    let mut passes: Vec<Box<dyn Pass>> = vec![
        Box::new(CsePass),
        Box::new(ConstFoldPass),
        Box::new(LicmPass),
        // Escape always — ReprSelect and future codegen consume `CoreFun::escaping`.
        Box::new(EscapePass),
    ];
    if opts.release {
        passes.push(Box::new(InlinePass));
        // Inline exposes fresh literals / builtins — fold again before fusion/repr.
        passes.push(Box::new(ConstFoldPass));
        passes.push(Box::new(ConcatIdentPass));
        passes.push(Box::new(ReprSelect));
        passes.push(Box::new(CopyElimPass));
    } else {
        passes.push(Box::new(ReprSelect));
    }

    for p in passes {
        p.run(module);
    }

    if let Some(plan) = memo_plan {
        apply_memo_plan(module, &plan);
    }
}

/// Named passes for tooling / diagnostics.
///
/// `"memo_tf"` is listed for Release even though planning runs via [`plan_memo_tf`]
/// *before* CSE (not as a `Pass::run`); applying a fresh plan after CSE would
/// drop const-reuse evidence (§7.5.2).
pub fn pass_names(release: bool) -> Vec<&'static str> {
    if release {
        vec![
            "cse",
            "const_fold",
            "licm",
            "escape",
            "inline",
            "const_fold",
            "concat_ident",
            "repr_select",
            "copy_elim",
            "memo_tf",
        ]
    } else {
        vec!["cse", "const_fold", "licm", "escape", "repr_select"]
    }
}

/// CSE: identical pure expressions share one SSA local (§7.5.1-A).
struct CsePass;
impl Pass for CsePass {
    fn name(&self) -> &str {
        "cse"
    }
    fn run(&self, module: &mut CoreModule) {
        cse_module(module);
    }
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
    use super::*;
    use copy_elim::CopyElimPass;
    use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value};
    use lumia_ty::{Effect, Type};
    use repr_select::ReprSelect;
    use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

    #[test]
    fn defaults() {
        assert_eq!(default_list_repr(), ListRepr::HeapList);
        assert_eq!(default_map_repr(), MapRepr::HashOrdered);
    }

    #[test]
    fn pass_pipeline_names() {
        assert!(pass_names(true).contains(&"inline"));
        assert!(pass_names(true).contains(&"escape"));
        assert!(pass_names(true).contains(&"copy_elim"));
        assert!(pass_names(true).contains(&"const_fold"));
        assert!(pass_names(true).contains(&"licm"));
        assert!(pass_names(true).contains(&"concat_ident"));
        assert!(pass_names(true).contains(&"memo_tf"));
        assert!(!pass_names(false).contains(&"inline"));
        assert!(!pass_names(false).contains(&"memo_tf"));
    }

    #[test]
    fn copy_elim_collapses_alias() {
        let mut module = CoreModule {
            name: "M".into(),
            functions: vec![CoreFun {
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
            hash_adts: HashSet::default(),
            trait_methods: HashMap::default(),
        };
        CopyElimPass.run(&mut module);
        let f = &module.functions[0];
        assert_eq!(f.body.ops.len(), 1);
        assert_eq!(f.body.result, Some(Local(0)));
    }

    #[test]
    fn repr_select_marks_nonescaping_small_list_lit() {
        let mut module = CoreModule {
            name: "M".into(),
            functions: vec![CoreFun {
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
            hash_adts: HashSet::default(),
            trait_methods: HashMap::default(),
        };
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
        let mut module = CoreModule {
            name: "M".into(),
            functions: vec![CoreFun {
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
            hash_adts: HashSet::default(),
            trait_methods: HashMap::default(),
        };
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
        let mut core =
            lumia_core::lower_hir_with_schemes(&typed.module, &typed.fun_types, &typed.fun_schemes);
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
    fn repr_select_empty_list_is_lit() {
        let mut module = CoreModule {
            name: "M".into(),
            functions: vec![CoreFun {
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
            hash_adts: HashSet::default(),
            trait_methods: HashMap::default(),
        };
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

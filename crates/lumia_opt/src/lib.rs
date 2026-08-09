//! Optimization pass pipeline (§7.1 / §7.1.1).
//!
//! Transparent result reuse lives in [`memo`] (DESIGN §7.5):
//! local CSE/fold/LICM + runtime `T_f` (`memo_tf`).
//! Escape analysis + small pure inlining live in [`escape`] / [`inline`].

mod escape;
mod fusion;
mod inline;
mod memo;

pub use escape::{escaping_locals, EscapePass};
pub use fusion::FusionPass;
pub use inline::InlinePass;
pub use memo::{
    apply_memo_plan, plan_memo_tf, MemoL0Pass, MemoL1Pass, MemoTfPass, MEMO_IDX_CAP,
    MEMO_IDX_MAX_FUNS, MEMO_IDX_TABLE_BYTES, MEMO_L2_MAX_ARGS, MEMO_L2_MAX_FUNS, MEMO_L2_SLOTS,
    MEMO_PROCESS_BYTE_CAP, MEMO_SLOTS_TABLE_BYTES,
};

use lumia_core::{
    AdtRepr, Block, CoreFun, CoreModule, ListRepr, Local, MapRepr, Op, SetRepr, Value,
};
use memo::cse_module;
use std::collections::{HashMap, HashSet};

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
    let mut core = lumia_core::compile_source_to_core(src)?;
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

    // L0/L1 always: CSE + const-fold/copy-prop + LICM (semantic-preserving).
    let mut passes: Vec<Box<dyn Pass>> = vec![
        Box::new(CsePass),
        Box::new(MemoL0Pass),
        Box::new(MemoL1Pass),
        // Escape always — ReprSelect and future codegen consume `CoreFun::escaping`.
        Box::new(EscapePass),
    ];
    if opts.release {
        passes.push(Box::new(InlinePass));
        passes.push(Box::new(FusionPass));
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

/// Named passes exposed for tooling / `--show-passes` later.
pub fn pass_names(release: bool) -> Vec<&'static str> {
    if release {
        vec![
            "cse",
            "memo_l0",
            "memo_l1",
            "escape",
            "inline",
            "fusion",
            "repr_select",
            "copy_elim",
            "memo_tf",
        ]
    } else {
        vec!["cse", "memo_l0", "memo_l1", "escape", "repr_select"]
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

/// Copy elimination: collapse `let x = y` aliases (SSA copy-prop).
struct CopyElimPass;
impl Pass for CopyElimPass {
    fn name(&self) -> &str {
        "copy_elim"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            elim_copies_in_fun(f);
        }
    }
}

fn elim_copies_in_fun(f: &mut CoreFun) {
    let mut remap: HashMap<u32, u32> = HashMap::new();
    collect_copy_aliases(&f.body, &mut remap);
    if remap.is_empty() {
        return;
    }
    // Flatten chains a→b→c to a→c.
    let keys: Vec<u32> = remap.keys().copied().collect();
    for k in keys {
        let mut cur = k;
        let mut seen = HashSet::new();
        while let Some(&n) = remap.get(&cur) {
            if !seen.insert(cur) {
                break;
            }
            cur = n;
        }
        remap.insert(k, cur);
    }
    apply_local_remap(&mut f.body, &remap);
    strip_identity_lets(&mut f.body, &remap);
}

fn collect_copy_aliases(block: &Block, remap: &mut HashMap<u32, u32>) {
    for op in &block.ops {
        match op {
            Op::Let {
                local,
                value: Value::Local(src),
                ..
            } => {
                remap.insert(local.0, src.0);
            }
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                collect_copy_aliases_value(value, remap);
            }
            _ => {}
        }
    }
}

fn collect_copy_aliases_value(value: &Value, remap: &mut HashMap<u32, u32>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_copy_aliases(then_block, remap);
            collect_copy_aliases(else_block, remap);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_copy_aliases(header, remap);
            collect_copy_aliases(body, remap);
            collect_copy_aliases(latch, remap);
        }
        Value::Lambda { body, .. } => collect_copy_aliases(body, remap),
        _ => {}
    }
}

fn apply_local_remap(block: &mut Block, remap: &HashMap<u32, u32>) {
    let map_l = |l: &mut Local| {
        if let Some(&r) = remap.get(&l.0) {
            *l = Local(r);
        }
    };
    if let Some(r) = &mut block.result {
        map_l(r);
    }
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                remap_value_locals(value, remap);
            }
            Op::Assign { value, .. } | Op::Return { value } => map_l(value),
            Op::Break | Op::Continue => {}
        }
    }
}

fn remap_value_locals(value: &mut Value, remap: &HashMap<u32, u32>) {
    lumia_core::map_value_locals(
        value,
        &mut |l| {
            if let Some(&r) = remap.get(&l.0) {
                *l = Local(r);
            }
        },
        &mut |b| apply_local_remap(b, remap),
    );
}

fn strip_identity_lets(block: &mut Block, aliases: &HashMap<u32, u32>) {
    block.ops.retain(|op| {
        !matches!(
            op,
            Op::Let { local, .. } if aliases.contains_key(&local.0)
        )
    });
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                strip_identity_lets_value(value, aliases);
            }
            _ => {}
        }
    }
}

fn strip_identity_lets_value(value: &mut Value, aliases: &HashMap<u32, u32>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            strip_identity_lets(then_block, aliases);
            strip_identity_lets(else_block, aliases);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            strip_identity_lets(header, aliases);
            strip_identity_lets(body, aliases);
            strip_identity_lets(latch, aliases);
        }
        Value::Lambda { body, .. } => strip_identity_lets(body, aliases),
        _ => {}
    }
}

/// Representation selection: prove → specialize; else default (§7.1.1).
struct ReprSelect;
impl Pass for ReprSelect {
    fn name(&self) -> &str {
        "repr_select"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            // EscapePass always runs first and fills `f.escaping`.
            let escaping = f.escaping.clone();
            select_in_fun(f, &escaping);
        }
    }
}

fn select_in_fun(f: &mut CoreFun, escaping: &HashSet<Local>) {
    for op in &mut f.body.ops {
        if let Op::Let { local, value, .. } = op {
            select_value(value, *local, escaping);
        }
    }
}

fn select_value(v: &mut Value, bound: Local, escaping: &HashSet<Local>) {
    let local_ok = !escaping.contains(&bound);
    match v {
        Value::AllocList { elems, repr } => {
            if elems.is_empty() {
                // Empty → immortal singleton (`lumia_list_empty`).
                *repr = ListRepr::LitList;
            } else if local_ok && elems.len() <= 8 {
                // Non-escaping small literal → stack layout in codegen (DESIGN §7).
                *repr = ListRepr::LitList;
            } else {
                *repr = ListRepr::HeapList;
            }
        }
        Value::AllocMap { flat_pairs, repr } => {
            let n_pairs = flat_pairs.len() / 2;
            // Preserve Eq-only AssocList; else stack LitMap when non-escaping ≤8.
            if matches!(*repr, MapRepr::AssocList) {
                // keep
            } else if local_ok && n_pairs > 0 && n_pairs <= 8 {
                *repr = MapRepr::LitMap;
            } else if n_pairs <= 8 {
                *repr = MapRepr::SmallMap;
            } else {
                *repr = default_map_repr();
            }
            let _ = flat_pairs;
        }
        Value::AllocSet { elems, repr } => {
            if local_ok && !elems.is_empty() && elems.len() <= 8 {
                *repr = SetRepr::LitSet;
            } else {
                *repr = SetRepr::HeapSet;
            }
        }
        Value::AllocAdt { fields, repr, .. } => {
            if local_ok && fields.len() <= 8 {
                *repr = AdtRepr::LitAdt;
            } else {
                *repr = AdtRepr::HeapAdt;
            }
        }
        Value::AllocClosure { .. } | Value::ClosureCap { .. } => {}
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            for op in then_block.ops.iter_mut().chain(else_block.ops.iter_mut()) {
                if let Op::Let { local, value, .. } = op {
                    select_value(value, *local, escaping);
                }
            }
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            for b in [&mut **header, &mut **body, &mut **latch] {
                for op in &mut b.ops {
                    if let Op::Let { local, value, .. } = op {
                        select_value(value, *local, escaping);
                    }
                }
            }
        }
        _ => {}
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
    use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value};
    use lumia_ty::{Effect, Type};

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
        assert!(!pass_names(false).contains(&"inline"));
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
                escaping: std::collections::HashSet::new(),
                scheme_poly: false,
            }],
            hash_adts: std::collections::HashSet::new(),
            trait_methods: std::collections::HashMap::new(),
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
                escaping: std::collections::HashSet::new(),
                scheme_poly: false,
            }],
            hash_adts: std::collections::HashSet::new(),
            trait_methods: std::collections::HashMap::new(),
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
                escaping: std::collections::HashSet::new(),
                scheme_poly: false,
            }],
            hash_adts: std::collections::HashSet::new(),
            trait_methods: std::collections::HashMap::new(),
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
                escaping: std::collections::HashSet::new(),
                scheme_poly: false,
            }],
            hash_adts: std::collections::HashSet::new(),
            trait_methods: std::collections::HashMap::new(),
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

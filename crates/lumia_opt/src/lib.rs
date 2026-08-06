//! Optimization pass pipeline (§7.1 / §7.1.1).
//!
//! Interface is final-form; individual passes start thin and grow.

use lumia_core::{Block, CoreFun, CoreModule, ListRepr, MapRepr, Op, Value};
use std::collections::HashMap;

pub struct OptOptions {
    pub release: bool,
}

impl Default for OptOptions {
    fn default() -> Self {
        Self { release: false }
    }
}

pub trait Pass {
    fn name(&self) -> &str;
    fn run(&self, module: &mut CoreModule);
}

/// Run the standard pipeline. Uncertain → default stable paths (§7.1.1).
pub fn optimize(module: &mut CoreModule, opts: &OptOptions) {
    let passes: Vec<Box<dyn Pass>> = if opts.release {
        vec![
            Box::new(InlineStub),
            Box::new(CsePass),
            Box::new(EscapeStub),
            Box::new(FusionStub),
            Box::new(ReprSelect),
            Box::new(CopyElimStub),
            Box::new(MemoL0Pass),
        ]
    } else {
        // Debug: fewer passes; still run CSE (safe) + default repr selection
        vec![Box::new(CsePass), Box::new(ReprSelect)]
    };

    for p in passes {
        p.run(module);
    }
}

/// Named passes exposed for tooling / `--show-passes` later.
pub fn pass_names(release: bool) -> Vec<&'static str> {
    if release {
        vec![
            "inline",
            "cse",
            "escape",
            "fusion",
            "repr_select",
            "copy_elim",
            "memo_l0",
        ]
    } else {
        vec!["cse", "repr_select"]
    }
}

struct InlineStub;
impl Pass for InlineStub {
    fn name(&self) -> &str {
        "inline"
    }
    fn run(&self, _module: &mut CoreModule) {}
}

/// Local CSE on pure ops: identical `i64` literals and pure binary of same locals.
struct CsePass;
impl Pass for CsePass {
    fn name(&self) -> &str {
        "cse"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            cse_block(&mut f.body);
        }
    }
}

fn cse_block(block: &mut Block) {
    let mut int_lit: HashMap<i64, u32> = HashMap::new();
    let mut rewrite: HashMap<u32, u32> = HashMap::new();

    for op in &mut block.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } if *pure_region => {
                // Rewrite operands first
                rewrite_value(value, &rewrite);
                if let Value::Int(n) = value {
                    if let Some(&prev) = int_lit.get(n) {
                        rewrite.insert(local.0, prev);
                        *value = Value::Local(lumia_core::Local(prev));
                    } else {
                        int_lit.insert(*n, local.0);
                    }
                }
            }
            Op::Let { value, .. } => {
                rewrite_value(value, &rewrite);
                if let Value::If {
                    then_block,
                    else_block,
                    ..
                } = value
                {
                    cse_block(then_block);
                    cse_block(else_block);
                }
                if let Value::Loop {
                    header,
                    body,
                    latch,
                } = value
                {
                    cse_block(header);
                    cse_block(body);
                    cse_block(latch);
                }
            }
            Op::Effect { value } => rewrite_value(value, &rewrite),
            Op::Assign { value, .. } => {
                if let Some(&r) = rewrite.get(&value.0) {
                    *value = lumia_core::Local(r);
                }
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = block.result {
        if let Some(&nr) = rewrite.get(&r.0) {
            block.result = Some(lumia_core::Local(nr));
        }
    }
}

fn rewrite_value(v: &mut Value, rewrite: &HashMap<u32, u32>) {
    let map_l = |l: &mut lumia_core::Local| {
        if let Some(&r) = rewrite.get(&l.0) {
            *l = lumia_core::Local(r);
        }
    };
    match v {
        Value::Local(l) => map_l(l),
        Value::Binary { left, right, .. } => {
            map_l(left);
            map_l(right);
        }
        Value::Unary { operand, .. } => map_l(operand),
        Value::Call { args, .. }
        | Value::Builtin { args, .. }
        | Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure {
            captures: args, ..
        } => {
            for a in args {
                map_l(a);
            }
        }
        Value::ClosureCap { env, .. } => map_l(env),
        Value::IndirectCall { callee, args } => {
            map_l(callee);
            for a in args {
                map_l(a);
            }
        }
        Value::If {
            cond,
            then_block,
            else_block,
        } => {
            map_l(cond);
            let _ = (then_block, else_block);
        }
        Value::Loop { header, body, latch } => {
            let _ = (header, body, latch);
        }
        Value::Lambda { .. }
        | Value::FunRef(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::Name(_) => {}
    }
}

struct EscapeStub;
impl Pass for EscapeStub {
    fn name(&self) -> &str {
        "escape"
    }
    fn run(&self, _module: &mut CoreModule) {}
}

struct FusionStub;
impl Pass for FusionStub {
    fn name(&self) -> &str {
        "fusion"
    }
    fn run(&self, _module: &mut CoreModule) {}
}

struct CopyElimStub;
impl Pass for CopyElimStub {
    fn name(&self) -> &str {
        "copy_elim"
    }
    fn run(&self, _module: &mut CoreModule) {}
}

/// L0: compile-time redundancy already handled by CSE; hook for future LICM.
struct MemoL0Pass;
impl Pass for MemoL0Pass {
    fn name(&self) -> &str {
        "memo_l0"
    }
    fn run(&self, _module: &mut CoreModule) {}
}

/// Representation selection: prove → specialize; else default (§7.1.1).
struct ReprSelect;
impl Pass for ReprSelect {
    fn name(&self) -> &str {
        "repr_select"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            select_in_fun(f);
        }
    }
}

fn select_in_fun(f: &mut CoreFun) {
    for op in &mut f.body.ops {
        if let Op::Let { value, .. } = op {
            select_value(value);
        }
    }
}

fn select_value(v: &mut Value) {
    match v {
        Value::AllocList { elems, repr } => {
            if elems.is_empty() {
                *repr = ListRepr::LitList;
            } else if elems.len() <= 8 {
                *repr = ListRepr::HeapList;
            } else {
                *repr = default_list_repr();
            }
        }
        Value::AllocMap { flat_pairs, repr } => {
            if flat_pairs.is_empty() {
                *repr = MapRepr::SmallMap;
            } else {
                *repr = default_map_repr();
            }
        }
        Value::AllocSet { .. } => {}
        Value::AllocAdt { .. } => {}
        Value::AllocClosure { .. } | Value::ClosureCap { .. } => {}
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            for op in then_block.ops.iter_mut().chain(else_block.ops.iter_mut()) {
                if let Op::Let { value, .. } = op {
                    select_value(value);
                }
            }
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            for op in header
                .ops
                .iter_mut()
                .chain(body.ops.iter_mut())
                .chain(latch.ops.iter_mut())
            {
                if let Op::Let { value, .. } = op {
                    select_value(value);
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
    use lumia_ty::Effect;
    use lumia_ty::Type;

    #[test]
    fn defaults() {
        assert_eq!(default_list_repr(), ListRepr::HeapList);
        assert_eq!(default_map_repr(), MapRepr::HashOrdered);
    }

    #[test]
    fn pass_pipeline_names() {
        assert!(pass_names(true).contains(&"repr_select"));
        assert!(pass_names(false).contains(&"cse"));
    }

    #[test]
    fn cse_dedups_int_literals() {
        let mut module = CoreModule {
            name: "C".into(),
            functions: vec![CoreFun {
                name: "main".into(),
                params: vec![],
                param_names: vec![],
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
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::Binary {
                                op: lumia_syntax::BinOp::Add,
                                left: Local(0),
                                right: Local(1),
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(2)),
                },
                ret_ty: Type::Int,
                effect: Effect::io(),
                is_main: true,
            }],
        };
        optimize(&mut module, &OptOptions { release: false });
        let real_ints = module.functions[0]
            .body
            .ops
            .iter()
            .filter(|op| matches!(op, Op::Let { value: Value::Int(1), .. }))
            .count();
        assert_eq!(real_ints, 1);
    }
}

//! Dead-code elimination for pure, non-trapping lets (DESIGN §7.2).
//!
//! Keeps anything that may trap or observe mutation (§2.4): Int arith / Neg,
//! effectful / trapping builtins, calls, allocs, and control flow. Unused
//! Float arith, comparisons, literals, and Local copies may be dropped.
//!
//! Liveness walks **nested** `If`/`Loop` bodies — shallow `for_each_local` alone
//! would drop loop-carried temps (e.g. `let z = 0` only read inside a loop).

use crate::ir_util::collect_float_locals;
use lumia_core::{
    for_each_local, for_each_nested_block, for_each_nested_block_mut, Block, CoreFun, CoreModule,
    Op, Value,
};
use lumia_syntax::{BinOp, UnOp};
use rustc_hash::FxHashSet as HashSet;

pub(crate) struct DcePass;
impl DcePass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        for f in &mut module.functions {
            if f.external.is_some() {
                continue;
            }
            dce_fun(f);
        }
    }
}

fn dce_fun(f: &mut CoreFun) {
    let mut float_locals = HashSet::default();
    for (i, ty) in f.param_tys.iter().enumerate() {
        if matches!(ty, lumia_ty::Type::Float) {
            if let Some(p) = f.params.get(i) {
                float_locals.insert(p.0);
            }
        }
    }
    collect_float_locals(&f.body, &mut float_locals);
    dce_block(&mut f.body, &float_locals);
}

fn dce_block(block: &mut Block, float_locals: &HashSet<u32>) {
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } => {
                for_each_nested_block_mut(value, &mut |nested| {
                    dce_block(nested, float_locals);
                });
            }
            _ => {}
        }
    }

    let mut live = HashSet::default();
    collect_references(block, &mut live);

    // Trapping / effectful lets stay even if their SSA name is unread.
    let mut changed = true;
    while changed {
        changed = false;
        for op in &block.ops {
            if let Op::Let { local, value, .. } = op {
                if must_keep(value, float_locals) && live.insert(local.0) {
                    changed = true;
                }
                if live.contains(&local.0) {
                    let before = live.len();
                    mark_uses_shallow(value, &mut live);
                    if live.len() != before {
                        changed = true;
                    }
                }
            }
        }
    }

    block.ops.retain(|op| match op {
        Op::Let { local, value, .. } => {
            live.contains(&local.0) || must_keep(value, float_locals)
        }
        _ => true,
    });
}

/// Deep: every Local operand and block result under `block` (including nested).
fn collect_references(block: &Block, live: &mut HashSet<u32>) {
    if let Some(r) = block.result {
        live.insert(r.0);
    }
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                mark_uses_shallow(value, live);
                for_each_nested_block(value, &mut |nested| {
                    collect_references(nested, live);
                });
            }
            Op::Assign { value, .. } | Op::Return { value } => {
                live.insert(value.0);
            }
            Op::Break | Op::Continue => {}
        }
    }
}

fn mark_uses_shallow(value: &Value, used: &mut HashSet<u32>) {
    for_each_local(value, &mut |l| {
        used.insert(l.0);
    });
}

fn must_keep(value: &Value, float_locals: &HashSet<u32>) -> bool {
    match value {
        Value::If { .. } | Value::Loop { .. } | Value::Lambda { .. } => true,
        Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. } => true,
        Value::Call { .. } | Value::IndirectCall { .. } => true,
        Value::Name(_) => true,
        Value::Binary {
            op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
            left,
            right,
        } => !(float_locals.contains(&left.0) && float_locals.contains(&right.0)),
        Value::Unary {
            op: UnOp::Neg,
            operand,
        } => !float_locals.contains(&operand.0),
        Value::Builtin { name, .. } => crate::memo::builtin_may_trap_or_effect(name),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value, FunKind};
    use lumia_syntax::BinOp;
    use lumia_ty::Effect;

    fn bare_fun(name: &str, ops: Vec<Op>, result: Option<Local>) -> CoreFun {
        CoreFun {
            name: name.into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            ret_ty: lumia_ty::Type::Int,
            effect: Effect::pure(),
            body: Block {
                ops,
                result,
            },
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        }
    }

    #[test]
    fn drops_unused_pure_literal() {
        let mut module = CoreModule::with_functions(
            "D",
            vec![bare_fun(
                "f",
                vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                ],
                Some(Local(1)),
            )],
        );
        DcePass.run(&mut module);
        let ops = &module.functions[0].body.ops;
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            &ops[0],
            Op::Let {
                local: Local(1),
                ..
            }
        ));
    }

    #[test]
    fn keeps_unused_int_div() {
        let mut module = CoreModule::with_functions(
            "D",
            vec![bare_fun(
                "f",
                vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(0),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Binary {
                            op: BinOp::Div,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Int(9),
                        pure_region: true,
                    },
                ],
                Some(Local(3)),
            )],
        );
        DcePass.run(&mut module);
        let ops = &module.functions[0].body.ops;
        assert!(
            ops.iter().any(|op| matches!(
                op,
                Op::Let {
                    value: Value::Binary {
                        op: BinOp::Div,
                        ..
                    },
                    ..
                }
            )),
            "unused Int Div must remain (may trap): {ops:?}"
        );
    }

    #[test]
    fn keeps_temp_only_used_inside_loop() {
        // `%0 = 0` assigned into a slot read only in the loop body.
        let loop_body = Block {
            ops: vec![Op::Assign {
                name: "acc".into(),
                value: Local(0),
            }],
            result: None,
        };
        let mut module = CoreModule::with_functions(
            "D",
            vec![bare_fun(
                "f",
                vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(0),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Loop {
                            header: Box::new(Block {
                                ops: vec![],
                                result: Some(Local(0)),
                            }),
                            body: Box::new(loop_body),
                            latch: Box::new(Block {
                                ops: vec![],
                                result: None,
                            }),
                        },
                        pure_region: false,
                    },
                ],
                Some(Local(1)),
            )],
        );
        DcePass.run(&mut module);
        let ops = &module.functions[0].body.ops;
        assert!(
            ops.iter().any(|op| matches!(
                op,
                Op::Let {
                    local: Local(0),
                    value: Value::Int(0),
                    ..
                }
            )),
            "loop-only temp must survive: {ops:?}"
        );
    }
}

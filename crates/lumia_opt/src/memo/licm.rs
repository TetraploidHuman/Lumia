use lumia_core::{Block, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
use rustc_hash::FxHashSet as HashSet;

pub(crate) fn licm_block(block: &mut Block) {
    let mut out = Vec::with_capacity(block.ops.len());
    for mut op in std::mem::take(&mut block.ops) {
        match &mut op {
            Op::Let {
                value:
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    },
                ..
            } => {
                licm_block(then_block);
                licm_block(else_block);
                out.push(op);
            }
            Op::Let {
                value:
                    Value::Loop {
                        header,
                        body,
                        latch,
                    },
                ..
            } => {
                licm_block(header);
                licm_block(body);
                licm_block(latch);

                let mut loop_defs = HashSet::default();
                collect_defs(header, &mut loop_defs);
                collect_defs(body, &mut loop_defs);
                collect_defs(latch, &mut loop_defs);

                // Iterate: hoist pure lets whose operands are all outside the loop.
                loop {
                    let mut changed = false;
                    let mut kept = Vec::new();
                    for hop in std::mem::take(&mut body.ops) {
                        if let Op::Let {
                            local,
                            value,
                            pure_region: true,
                        } = &hop
                        {
                            if is_hoistable(value, &loop_defs) {
                                loop_defs.remove(&local.0);
                                out.push(hop);
                                changed = true;
                                continue;
                            }
                        }
                        kept.push(hop);
                    }
                    body.ops = kept;
                    if !changed {
                        break;
                    }
                }
                out.push(op);
            }
            _ => out.push(op),
        }
    }
    block.ops = out;
}

fn collect_defs(block: &Block, defs: &mut HashSet<u32>) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defs.insert(local.0);
                match value {
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        collect_defs(then_block, defs);
                        collect_defs(else_block, defs);
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        collect_defs(header, defs);
                        collect_defs(body, defs);
                        collect_defs(latch, defs);
                    }
                    _ => {}
                }
            }
            Op::Assign { .. }
            | Op::Effect { .. }
            | Op::Break
            | Op::Continue
            | Op::Return { .. } => {}
        }
    }
}

fn is_hoistable(value: &Value, loop_defs: &HashSet<u32>) -> bool {
    match value {
        // Don't hoist control / alloc / names (may observe mutation).
        Value::If { .. }
        | Value::Loop { .. }
        | Value::Lambda { .. }
        | Value::Name(_)
        | Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. }
        | Value::IndirectCall { .. } => false,
        // Checked Int arithmetic / Neg may trap — must not hoist past break (§2.4).
        Value::Binary {
            op: BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
            ..
        } => false,
        Value::Unary { op: UnOp::Neg, .. } => false,
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_) => true,
        Value::Local(l) => !loop_defs.contains(&l.0),
        Value::Unary { operand, .. } => !loop_defs.contains(&operand.0),
        Value::Binary { left, right, .. } => {
            !loop_defs.contains(&left.0) && !loop_defs.contains(&right.0)
        }
        // User calls / builtins may trap or allocate; only hoist trivial locals above.
        Value::Call { .. } => false,
        Value::Builtin { name, args } => {
            if builtin_may_trap_or_effect(name) {
                return false;
            }
            args.iter().all(|a| !loop_defs.contains(&a.0))
        }
        Value::ClosureCap { env, .. } => !loop_defs.contains(&env.0),
    }
}

pub(super) fn builtin_may_trap_or_effect(b: &Builtin) -> bool {
    matches!(
        b,
        Builtin::ListGet
            | Builtin::MapRemove
            | Builtin::Println
            | Builtin::PrintlnInt
            | Builtin::PrintlnStr
            | Builtin::ReadStdin
            | Builtin::MatchFail
            | Builtin::Assert
            | Builtin::ListParMap
            | Builtin::ListParFold
            | Builtin::Range
            | Builtin::RangeInclusive
            | Builtin::AdtField
            | Builtin::AdtTag
    )
}

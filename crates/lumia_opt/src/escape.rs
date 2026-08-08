//! Escape analysis (DESIGN §7.2 Escape Analysis).
//!
//! Conservative: a local escapes if it may be observed after the current
//! function returns, stored into a heap object that escapes, passed to an
//! unknown callee, or written into a named binding.

use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use std::collections::HashSet;

/// Locals that may outlive their defining region / be observed from outside.
pub fn escaping_locals(fun: &CoreFun) -> HashSet<Local> {
    let mut escaping: HashSet<Local> = HashSet::new();
    seed_escaping(&fun.body, &mut escaping);
    // Fixed-point: aliases + heap stores propagate escape.
    let mut changed = true;
    while changed {
        changed = false;
        changed |= propagate_block(&fun.body, &mut escaping);
    }
    escaping
}

/// Escape analysis: write results onto each [`CoreFun::escaping`] for later passes.
pub struct EscapePass;

impl crate::Pass for EscapePass {
    fn name(&self) -> &str {
        "escape"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            f.escaping = escaping_locals(f);
        }
    }
}

fn seed_escaping(block: &Block, escaping: &mut HashSet<Local>) {
    if let Some(r) = block.result {
        escaping.insert(r);
    }
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => seed_value(value, escaping),
            Op::Assign { value, .. } => {
                // Named bindings are visible across the function; treat as escaping.
                escaping.insert(*value);
            }
            Op::Break | Op::Continue => {}
        }
    }
}

fn seed_value(value: &Value, escaping: &mut HashSet<Local>) {
    match value {
        Value::Call { args, .. } | Value::IndirectCall { args, .. } => {
            for a in args {
                escaping.insert(*a);
            }
            if let Value::IndirectCall { callee, .. } = value {
                escaping.insert(*callee);
            }
        }
        Value::Builtin { name, args } => {
            if builtin_may_capture(*name) {
                for a in args {
                    escaping.insert(*a);
                }
            }
        }
        Value::FunRef(_) => {}
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            seed_escaping(then_block, escaping);
            seed_escaping(else_block, escaping);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            seed_escaping(header, escaping);
            seed_escaping(body, escaping);
            seed_escaping(latch, escaping);
        }
        Value::Lambda { body, .. } => seed_escaping(body, escaping),
        _ => {}
    }
}

fn builtin_may_capture(b: Builtin) -> bool {
    // Conservative: anything that builds / mutates heap collections or does I/O
    // with a value may retain it. Pure projections (len/get/tag) do not.
    !matches!(
        b,
        Builtin::ListLen
            | Builtin::ListGet
            | Builtin::AdtTag
            | Builtin::AdtField
            | Builtin::Contains
            | Builtin::Show
            | Builtin::MatchFail
    )
}

fn propagate_block(block: &Block, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                changed |= propagate_let(*local, value, escaping);
            }
            Op::Effect { value } => {
                changed |= propagate_value_only(value, escaping);
            }
            Op::Assign { .. } | Op::Break | Op::Continue => {}
        }
    }
    changed
}

fn propagate_let(local: Local, value: &Value, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    // If the binding escapes, everything it aliases / contains escapes.
    if escaping.contains(&local) {
        changed |= mark_inputs_escaping(value, escaping);
    }
    // If a heap object escapes, its payload locals escape (already in mark_inputs).
    // If payload escapes and is stored into alloc that is used as `local`, local escapes
    // when any elem escapes? For immutable lists, storing a non-escaping elem into a
    // non-escaping list is fine — only reverse direction matters (container escapes ⇒ elems).
    changed |= propagate_value_only(value, escaping);
    changed
}

fn propagate_value_only(value: &Value, escaping: &mut HashSet<Local>) -> bool {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            let mut c = propagate_block(then_block, escaping);
            c |= propagate_block(else_block, escaping);
            c
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            let mut c = propagate_block(header, escaping);
            c |= propagate_block(body, escaping);
            c |= propagate_block(latch, escaping);
            c
        }
        Value::Lambda { body, .. } => propagate_block(body, escaping),
        _ => false,
    }
}

fn mark_inputs_escaping(value: &Value, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    let mut mark = |l: Local| {
        if escaping.insert(l) {
            changed = true;
        }
    };
    match value {
        Value::Local(l) => mark(*l),
        Value::Binary { left, right, .. } => {
            mark(*left);
            mark(*right);
        }
        Value::Unary { operand, .. } => mark(*operand),
        Value::Builtin { name, args } => {
            // Pure projections do not retain the collection; returning `xs.len()`
            // must not mark `xs` itself as escaping.
            if builtin_may_capture(*name) {
                for a in args {
                    mark(*a);
                }
            }
        }
        Value::Call { args, .. }
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
                mark(*a);
            }
        }
        Value::IndirectCall { callee, args } => {
            mark(*callee);
            for a in args {
                mark(*a);
            }
        }
        Value::ClosureCap { env, .. } => mark(*env),
        Value::If { cond, .. } => mark(*cond),
        Value::Name(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_)
        | Value::Loop { .. }
        | Value::Lambda { .. } => {}
    }
    changed
}

/// True when `local` is not in the escaping set (definitely local).
pub fn is_non_escaping(escaping: &HashSet<Local>, local: Local) -> bool {
    !escaping.contains(&local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{Block, CoreFun, Op, Value};
    use lumia_ty::{Effect, Type};

    fn fun_with_body(body: Block) -> CoreFun {
        CoreFun {
            name: "f".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body,
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
        escaping: std::collections::HashSet::new(),
        }
    }

    #[test]
    fn result_escapes() {
        let body = Block {
            params: vec![],
            ops: vec![Op::Let {
                local: Local(0),
                value: Value::Int(1),
                pure_region: true,
            }],
            result: Some(Local(0)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(0)));
    }

    #[test]
    fn dead_temp_does_not_escape() {
        let body = Block {
            params: vec![],
            ops: vec![
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
            result: Some(Local(1)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(1)));
        assert!(!esc.contains(&Local(0)));
    }

    #[test]
    fn call_args_escape() {
        let body = Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Call {
                        fun: "g".into(),
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(1)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(0)));
        assert!(esc.contains(&Local(1)));
    }

    #[test]
    fn alloc_elems_escape_when_list_returned() {
        let body = Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(7),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::AllocList {
                        elems: vec![Local(0)],
                        repr: lumia_core::ListRepr::HeapList,
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(1)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(1)));
        assert!(esc.contains(&Local(0)));
    }
}

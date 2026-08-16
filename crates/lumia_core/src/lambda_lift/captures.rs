//! Capture / free-variable analysis for nested lambdas.

use crate::ir::{Block, Local, Op, Value};
use crate::visit::{for_each_local, for_each_nested_block};
use rustc_hash::FxHashSet as HashSet;

pub(super) fn analyze_captures(body: &Block, params: &[Local]) -> (Vec<Local>, Vec<String>) {
    let mut defined = HashSet::default();
    for p in params {
        defined.insert(p.0);
    }
    collect_defined_locals(body, &mut defined);
    let mut used_locals = HashSet::default();
    let mut free_names = HashSet::default();
    let mut bound_names = HashSet::default();
    collect_free(body, &mut used_locals, &mut bound_names, &mut free_names);
    let mut free_locals: Vec<Local> = used_locals
        .into_iter()
        .filter(|id| !defined.contains(id))
        .map(Local)
        .collect();
    free_locals.sort_by_key(|l| l.0);
    let mut free_names: Vec<String> = free_names.into_iter().collect();
    free_names.sort();
    (free_locals, free_names)
}

fn collect_defined_locals(block: &Block, defined: &mut HashSet<u32>) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defined.insert(local.0);
                collect_defined_in_value(value, defined);
            }
            Op::Assign { .. } | Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

fn collect_defined_in_value(value: &Value, defined: &mut HashSet<u32>) {
    if let Value::Lambda { params, .. } = value {
        for p in params {
            defined.insert(p.0);
        }
    }
    for_each_nested_block(value, &mut |b| collect_defined_locals(b, defined));
}

/// Walk in program order: `Assign` binds a mutable name for subsequent uses.
/// A `Name` load is free only when that name is not yet bound in this lambda.
fn collect_free(
    block: &Block,
    used_locals: &mut HashSet<u32>,
    bound_names: &mut HashSet<String>,
    free_names: &mut HashSet<String>,
) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } => {
                collect_free_in_value(value, used_locals, bound_names, free_names);
            }
            Op::Assign { name, value } => {
                // RHS is a Local (load of outer `n` is a prior `Let` with `Name`).
                used_locals.insert(value.0);
                bound_names.insert(name.clone());
            }
            Op::Return { value } => {
                used_locals.insert(value.0);
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = &block.result {
        used_locals.insert(r.0);
    }
}

fn collect_free_in_value(
    value: &Value,
    used_locals: &mut HashSet<u32>,
    bound_names: &mut HashSet<String>,
    free_names: &mut HashSet<String>,
) {
    for_each_local(value, &mut |l| {
        used_locals.insert(l.0);
    });
    if let Value::Name(n) = value {
        if !bound_names.contains(n) {
            free_names.insert(n.clone());
        }
    }
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            // Assignments in one branch should not bind the other; after the if,
            // names assigned in either branch are bound for subsequent code.
            let mut then_bound = bound_names.clone();
            let mut else_bound = bound_names.clone();
            collect_free(then_block, used_locals, &mut then_bound, free_names);
            collect_free(else_block, used_locals, &mut else_bound, free_names);
            for n in then_bound {
                bound_names.insert(n);
            }
            for n in else_bound {
                bound_names.insert(n);
            }
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            // Loop vars are initialized by `Assign` *before* the `Loop` value.
            collect_free(header, used_locals, bound_names, free_names);
            collect_free(body, used_locals, bound_names, free_names);
            collect_free(latch, used_locals, bound_names, free_names);
        }
        Value::Lambda { body, .. } => {
            let mut nested_bound = HashSet::default();
            collect_free(body, used_locals, &mut nested_bound, free_names);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Block, Local, Op, Value};

    fn empty_block() -> Block {
        Block {
            ops: vec![],
            result: None,
        }
    }

    #[test]
    fn local_mut_loop_counter_is_not_captured() {
        // `__i := 1; loop { load __i; ... }` — classic `for` lowering.
        let body = Block {
            ops: vec![
                Op::Assign {
                    name: "__i".into(),
                    value: Local(0),
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Loop {
                        header: Box::new(Block {
                            ops: vec![Op::Let {
                                local: Local(2),
                                value: Value::Name("__i".into()),
                                pure_region: true,
                            }],
                            result: Some(Local(2)),
                        }),
                        body: Box::new(empty_block()),
                        latch: Box::new(Block {
                            ops: vec![Op::Assign {
                                name: "__i".into(),
                                value: Local(3),
                            }],
                            result: None,
                        }),
                    },
                    pure_region: true,
                },
            ],
            result: None,
        };
        let (_, free_names) = analyze_captures(&body, &[]);
        assert!(free_names.is_empty(), "{free_names:?}");
    }

    #[test]
    fn outer_mut_load_is_captured() {
        let body = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Name("n".into()),
                    pure_region: true,
                },
                Op::Assign {
                    name: "n".into(),
                    value: Local(0),
                },
            ],
            result: None,
        };
        let (_, free_names) = analyze_captures(&body, &[Local(10)]);
        assert_eq!(free_names, vec!["n".to_string()]);
    }
}

//! Capture / free-variable analysis for nested lambdas.

use crate::ir::{Block, Local, Op, Value};
use crate::visit::{collect_defined_locals, for_each_local};
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
#[path = "captures_tests.rs"]
mod tests;

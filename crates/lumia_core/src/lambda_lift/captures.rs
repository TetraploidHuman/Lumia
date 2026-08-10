//! Capture / free-variable analysis for nested lambdas.

use crate::ir::{Block, Local, Op, Value};
use crate::visit::{collect_uses, for_each_nested_block};
use rustc_hash::FxHashSet as HashSet;

pub(super) fn analyze_captures(body: &Block, params: &[Local]) -> (Vec<Local>, Vec<String>) {
    let mut defined = HashSet::default();
    for p in params {
        defined.insert(p.0);
    }
    collect_defined_locals(body, &mut defined);
    let mut used_locals = HashSet::default();
    let mut used_names = HashSet::default();
    collect_uses(body, &mut used_locals, &mut used_names);
    let mut free_locals: Vec<Local> = used_locals
        .into_iter()
        .filter(|id| !defined.contains(id))
        .map(Local)
        .collect();
    free_locals.sort_by_key(|l| l.0);
    let mut free_names: Vec<String> = used_names.into_iter().collect();
    free_names.sort();
    (free_locals, free_names)
}

fn collect_defined_locals(block: &Block, defined: &mut HashSet<u32>) {
    for p in &block.params {
        defined.insert(p.0);
    }
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defined.insert(local.0);
                collect_defined_in_value(value, defined);
            }
            Op::Effect { value, .. } => collect_defined_in_value(value, defined),
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

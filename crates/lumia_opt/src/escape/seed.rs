//! Seed escaping locals from returns, assigns, calls, and builtins.

use super::ParamEscape;
use lumia_core::{Block, Local, Op, Value};
use lumia_hir::Builtin;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) fn seed_escaping(
    block: &Block,
    escaping: &mut HashSet<Local>,
    summaries: &HashMap<String, ParamEscape>,
) {
    if let Some(r) = block.result {
        escaping.insert(r);
    }
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                seed_value(value, escaping, summaries)
            }
            Op::Assign { value, .. } => {
                // Named bindings are visible across the function; treat as escaping.
                escaping.insert(*value);
            }
            Op::Return { value } => {
                // Early return leaves the function — same as `block.result`.
                escaping.insert(*value);
            }
            Op::Break | Op::Continue => {}
        }
    }
}

fn seed_value(
    value: &Value,
    escaping: &mut HashSet<Local>,
    summaries: &HashMap<String, ParamEscape>,
) {
    match value {
        Value::Call { fun, args } => {
            if let Some(pe) = summaries.get(fun) {
                for (i, a) in args.iter().enumerate() {
                    // Missing summary slots → conservative escape.
                    if pe.get(i).copied().unwrap_or(true) {
                        escaping.insert(*a);
                    }
                }
            } else {
                for a in args {
                    escaping.insert(*a);
                }
            }
        }
        Value::IndirectCall { callee, args } => {
            escaping.insert(*callee);
            for a in args {
                escaping.insert(*a);
            }
        }
        Value::Builtin { name, args } => {
            if name.may_capture() {
                for a in args {
                    escaping.insert(*a);
                }
            } else if matches!(*name, Builtin::Show) {
                // `lumia_show` requires a heap payload; Lit* stack objects print as ints.
                for a in args {
                    escaping.insert(*a);
                }
            } else if matches!(*name, Builtin::ListGet | Builtin::Contains) {
                // Collection is not retained, but Map/Set *keys* must be heap
                // objects: `lumia_eq` rejects non-heap payloads (`is_heap_payload`).
                if let Some(k) = args.get(1) {
                    escaping.insert(*k);
                }
            }
        }
        Value::FunRef(_) => {}
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            seed_escaping(then_block, escaping, summaries);
            seed_escaping(else_block, escaping, summaries);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            seed_escaping(header, escaping, summaries);
            seed_escaping(body, escaping, summaries);
            seed_escaping(latch, escaping, summaries);
        }
        Value::Lambda { body, .. } => seed_escaping(body, escaping, summaries),
        _ => {}
    }
}

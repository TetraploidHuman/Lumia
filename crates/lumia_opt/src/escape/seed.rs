//! Seed escaping locals from returns, assigns, calls, and builtins.

use super::EscapeSummaries;
use lumia_core::{for_each_block_dfs, for_each_op_in_block, Block, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::Sym;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) fn seed_escaping(
    block: &Block,
    escaping: &mut HashSet<Local>,
    summaries: &EscapeSummaries,
    assigns: &HashMap<Sym, Vec<Local>>,
) {
    // Order-independent set inserts — DFS is safe.
    for_each_block_dfs(block, &mut |b| {
        if let Some(r) = b.result {
            escaping.insert(r);
        }
    });
    for_each_op_in_block(block, &mut |op| {
        match op {
            Op::Let { value, .. } => {
                seed_value_shallow(value, escaping, summaries, assigns);
            }
            Op::Assign { .. } => {
                // Not an automatic escape: short-lived `var xs = listOf(…)` can
                // stay Lit*. Escape via `Name` / return is handled in propagate.
            }
            Op::Return { value } => {
                escaping.insert(*value);
            }
            Op::Break | Op::Continue => {}
        }
    });
}

fn mark_name_assigns(
    name: &str,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<Sym, Vec<Local>>,
) {
    if let Some(ls) = assigns.get(name) {
        for l in ls {
            escaping.insert(*l);
        }
    }
}

/// Seed one value leaf; nested If/Loop/Lambda bodies are visited by DFS.
fn seed_value_shallow(
    value: &Value,
    escaping: &mut HashSet<Local>,
    summaries: &EscapeSummaries,
    assigns: &HashMap<Sym, Vec<Local>>,
) {
    match value {
        Value::Call { fun, args } => {
            // Prefer CallTarget.id; name fallback for unresolved callees.
            if let Some(pe) = summaries.lookup_call(fun) {
                for (i, a) in args.iter().enumerate() {
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
        Value::Builtin { name, args, .. } => {
            if name.may_capture() || matches!(*name, Builtin::Show) {
                for a in args {
                    escaping.insert(*a);
                }
            } else if matches!(*name, Builtin::ListGet | Builtin::Contains) {
                if let Some(k) = args.get(1) {
                    escaping.insert(*k);
                }
            }
        }
        Value::Name(n) => mark_name_assigns(n, escaping, assigns),
        Value::FunRef(_) | Value::If { .. } | Value::Loop { .. } | Value::Lambda { .. } => {}
        _ => {}
    }
}

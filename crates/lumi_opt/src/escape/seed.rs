//! Seed escaping locals from returns, assigns, calls, and builtins.

use super::ParamEscape;
use lumi_core::{Block, Local, Op, Value};
use lumi_hir::Builtin;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) fn collect_assigns(block: &Block, assigns: &mut HashMap<String, Vec<Local>>) {
    for op in &block.ops {
        match op {
            Op::Assign { name, value } => {
                assigns.entry(name.clone()).or_default().push(*value);
            }
            Op::Let { value, .. } | Op::Effect { value } => {
                collect_assigns_value(value, assigns);
            }
            _ => {}
        }
    }
}

fn collect_assigns_value(value: &Value, assigns: &mut HashMap<String, Vec<Local>>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_assigns(then_block, assigns);
            collect_assigns(else_block, assigns);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_assigns(header, assigns);
            collect_assigns(body, assigns);
            collect_assigns(latch, assigns);
        }
        Value::Lambda { body, .. } => collect_assigns(body, assigns),
        _ => {}
    }
}

pub(super) fn seed_escaping(
    block: &Block,
    escaping: &mut HashSet<Local>,
    summaries: &HashMap<String, ParamEscape>,
    assigns: &HashMap<String, Vec<Local>>,
) {
    if let Some(r) = block.result {
        escaping.insert(r);
    }
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                seed_value(value, escaping, summaries, assigns)
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
    }
}

fn mark_name_assigns(
    name: &str,
    escaping: &mut HashSet<Local>,
    assigns: &HashMap<String, Vec<Local>>,
) {
    if let Some(ls) = assigns.get(name) {
        for l in ls {
            escaping.insert(*l);
        }
    }
}

fn seed_value(
    value: &Value,
    escaping: &mut HashSet<Local>,
    summaries: &HashMap<String, ParamEscape>,
    assigns: &HashMap<String, Vec<Local>>,
) {
    match value {
        Value::Call { fun, args } => {
            if let Some(pe) = summaries.get(fun) {
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
        Value::Builtin { name, args } => {
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
        Value::FunRef(_) => {}
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            seed_escaping(then_block, escaping, summaries, assigns);
            seed_escaping(else_block, escaping, summaries, assigns);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            seed_escaping(header, escaping, summaries, assigns);
            seed_escaping(body, escaping, summaries, assigns);
            seed_escaping(latch, escaping, summaries, assigns);
        }
        Value::Lambda { body, .. } => seed_escaping(body, escaping, summaries, assigns),
        _ => {}
    }
}

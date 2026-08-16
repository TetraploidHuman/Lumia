//! Literals and variable references.

use super::super::ctx::CoreLowerCtx;
use crate::ir::{Local, Op, Value};
use lumia_hir::Expr as HirExpr;

pub(super) fn lower_lit(
    ctx: &mut CoreLowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::Int(n, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Int(*n),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Float(n, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Float(*n),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Bool(b, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Bool(*b),
                pure_region,
            });
            Some(l)
        }
        HirExpr::String(s, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::String(s.clone()),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Char(c, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Char(*c),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Unit(_) => None,
        HirExpr::Var(name, _) => {
            if ctx.mutables.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Name(name.clone()),
                    pure_region,
                });
                Some(l)
            } else if let Some(l) = ctx.name_to_local.get(name) {
                Some(*l)
            } else if let Some(synth) = prelude_ctor_funref(name) {
                // First-class / alias use: `val lo = listOf` → FunRef to a nullary
                // empty-alloc stub (call sites `listOf(…)` stay special-cased).
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::FunRef(synth.to_string()),
                    pure_region,
                });
                Some(l)
            } else if ctx.toplevel_funs.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::FunRef(name.clone()),
                    pure_region,
                });
                Some(l)
            } else if ctx.toplevel_vals.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Call {
                        fun: format!("__val_{name}"),
                        args: vec![],
                    },
                    pure_region,
                });
                Some(l)
            } else {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Name(name.clone()),
                    pure_region,
                });
                Some(l)
            }
        }
        _ => unreachable!("lower_lit: non-literal"),
    }
}

/// Synthetic Core names for first-class prelude collection constructors.
pub(super) fn prelude_ctor_funref(name: &str) -> Option<&'static str> {
    match name {
        "listOf" => Some("__prelude_listOf"),
        "mapOf" => Some("__prelude_mapOf"),
        "setOf" => Some("__prelude_setOf"),
        _ => None,
    }
}

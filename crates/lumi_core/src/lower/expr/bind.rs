//! Let / assign / seq / lambda bindings.

use super::super::ctx::CoreLowerCtx;
use super::{lower_expr, lower_expr_block};
use crate::ir::{Local, Op, Value};
use lumi_hir::Expr as HirExpr;

pub(super) fn lower_bind(
    ctx: &mut CoreLowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::Let {
            name,
            value,
            body,
            mutable,
            ..
        } => {
            let v = lower_expr(ctx, value, ops, pure_region);
            let saved = ctx.save_bindings();
            if let Some(l) = v {
                if *mutable {
                    ctx.bind_mutable(name.clone(), l);
                    ops.push(Op::Assign {
                        name: name.clone(),
                        value: l,
                    });
                } else {
                    // `val` may shadow an outer `var` for the duration of `body`.
                    ctx.mutables.remove(name);
                    ctx.bind_name(name.clone(), l);
                }
            }
            let result = lower_expr(ctx, body, ops, pure_region);
            ctx.restore_bindings(saved);
            result
        }
        HirExpr::Assign { name, value, .. } => {
            let v = match lower_expr(ctx, value, ops, pure_region) {
                Some(l) => l,
                None => {
                    // Unit RHS: materialize a 0 local so assign never panics.
                    let l = ctx.fresh();
                    ops.push(Op::Let {
                        local: l,
                        value: Value::Unit,
                        pure_region,
                    });
                    l
                }
            };
            if ctx.mutables.contains(name) {
                ops.push(Op::Assign {
                    name: name.clone(),
                    value: v,
                });
            } else {
                // Immutable binding: ty rejects user assigns; do not mutate an
                // outer `var` shadowed by `val` (and do not mark name mutable).
                ctx.bind_name(name.clone(), v);
            }
            None
        }
        HirExpr::Seq { stmts, .. } => {
            let mut last = None;
            for s in stmts {
                last = lower_expr(ctx, s, ops, pure_region);
            }
            last
        }
        HirExpr::Lambda { params, body, .. } => {
            let mut inner = CoreLowerCtx {
                next: ctx.next,
                name_to_local: ctx.name_to_local.clone(),
                mutables: ctx.mutables.clone(),
                toplevel_funs: ctx.toplevel_funs.clone(),
                toplevel_vals: ctx.toplevel_vals.clone(),
                trait_method_names: ctx.trait_method_names.clone(),
                io_funs: ctx.io_funs.clone(),
            };
            let mut pls = vec![];
            for p in params {
                let l = inner.fresh();
                inner.bind_name(p.clone(), l);
                pls.push(l);
            }
            let (block, _) = lower_expr_block(&mut inner, body);
            ctx.next = inner.next;
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Lambda {
                    params: pls,
                    body: Box::new(block),
                },
                pure_region,
            });
            Some(dest)
        }
        _ => unreachable!("lower_bind: non-binding"),
    }
}

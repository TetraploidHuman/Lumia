//! For-loop lowering helpers.

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use crate::ast::{Builtin, Expr};
use lumia_syntax::{BinOp, Span};

pub(crate) fn lower_for_in(
    ctx: &LowerCtx,
    binding: &lumia_syntax::ForBinding,
    iter: &lumia_syntax::Expr,
    body: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let body_e = lower_expr(ctx, body);
    match binding {
        lumia_syntax::ForBinding::Pair(k, v) => {
            let lowered = lower_expr(ctx, iter);
            // Map → MapItems; already a List[(K,V)] (listOf / items / sortBy) → as-is.
            // Runtime `lumia_map_items` is also identity on List as a safety net.
            let items = if expr_already_pair_list(&lowered) {
                lowered
            } else {
                Expr::BuiltinCall {
                    name: Builtin::MapItems,
                    args: vec![lowered],
                    span,
                }
            };
            let pair = format!("__kv_{}", span.start.0);
            let bind_k = Expr::Let {
                name: k.clone(),
                value: Box::new(Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![Expr::Var(pair.clone(), span), Expr::Int(0, span)],
                    span,
                }),
                body: Box::new(Expr::Let {
                    name: v.clone(),
                    value: Box::new(Expr::BuiltinCall {
                        name: Builtin::AdtField,
                        args: vec![Expr::Var(pair.clone(), span), Expr::Int(1, span)],
                        span,
                    }),
                    body: Box::new(body_e),
                    mutable: false,
                }),
                mutable: false,
            };
            list_for_in(ctx, &pair, items, bind_k, span)
        }
        lumia_syntax::ForBinding::Name(name) => {
            let lowered_iter = lower_expr(ctx, iter);
            if let Expr::BuiltinCall { name: b, args, .. } = &lowered_iter {
                if matches!(b, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
                    let inclusive = matches!(b, Builtin::RangeInclusive);
                    return counter_for_in(
                        ctx,
                        name,
                        args[0].clone(),
                        args[1].clone(),
                        inclusive,
                        body_e,
                        span,
                    );
                }
            }
            list_for_in(ctx, name, lowered_iter, body_e, span)
        }
    }
}

/// True when `e` is already a List of pairs (not a Map needing `items()`).
fn expr_already_pair_list(e: &Expr) -> bool {
    match e {
        Expr::BuiltinCall { name, .. } => {
            matches!(name, Builtin::MapItems | Builtin::ListSortByKeys)
        }
        Expr::Call { callee, .. } => {
            matches!(callee.as_ref(), Expr::Var(n, _) if n == "listOf")
        }
        Expr::Let { body, .. } => expr_already_pair_list(body),
        Expr::Seq { stmts, .. } => stmts.last().map(expr_already_pair_list).unwrap_or(false),
        _ => false,
    }
}

pub(crate) fn counter_for_in(
    _ctx: &LowerCtx,
    binding: &str,
    start: Expr,
    end: Expr,
    inclusive: bool,
    body: Expr,
    span: Span,
) -> Expr {
    let i = format!("__i_{}", span.start.0);
    let cmp = if inclusive { BinOp::Le } else { BinOp::Lt };
    let cond = Expr::Binary {
        op: cmp,
        left: Box::new(Expr::Var(i.clone(), span)),
        right: Box::new(end),
        span,
    };
    let step = Expr::Assign {
        name: i.clone(),
        value: Box::new(Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Var(i.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    };
    let body = Expr::Let {
        name: binding.into(),
        value: Box::new(Expr::Var(i.clone(), span)),
        body: Box::new(body),
        mutable: false,
    };
    Expr::Let {
        name: i,
        value: Box::new(start),
        body: Box::new(Expr::Loop {
            cond: Box::new(cond),
            body: Box::new(body),
            step: Some(Box::new(step)),
            span,
        }),
        mutable: true,
    }
}

pub(crate) fn list_for_in(
    _ctx: &LowerCtx,
    binding: &str,
    list: Expr,
    body: Expr,
    span: Span,
) -> Expr {
    let xs = format!("__xs_{}", span.start.0);
    let i = format!("__i_{}", span.start.0);
    let n = format!("__n_{}", span.start.0);
    // Map is key-addressed; normalize to an indexable List (keys) first.
    let list = Expr::BuiltinCall {
        name: Builtin::Elems,
        args: vec![list],
        span,
    };
    let cond = Expr::Binary {
        op: BinOp::Lt,
        left: Box::new(Expr::Var(i.clone(), span)),
        right: Box::new(Expr::Var(n.clone(), span)),
        span,
    };
    let get = Expr::BuiltinCall {
        name: Builtin::ListGet,
        args: vec![Expr::Var(xs.clone(), span), Expr::Var(i.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: i.clone(),
        value: Box::new(Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Var(i.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    };
    let body = Expr::Let {
        name: binding.into(),
        value: Box::new(get),
        body: Box::new(body),
        mutable: false,
    };
    let loop_e = Expr::Loop {
        cond: Box::new(cond),
        body: Box::new(body),
        step: Some(Box::new(step)),
        span,
    };
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: n,
            value: Box::new(Expr::BuiltinCall {
                name: Builtin::ListLen,
                args: vec![Expr::Var(xs, span)],
                span,
            }),
            body: Box::new(Expr::Let {
                name: i,
                value: Box::new(Expr::Int(0, span)),
                body: Box::new(loop_e),
                mutable: true,
            }),
            mutable: false,
        }),
        mutable: false,
    }
}

/// Iterate with a custom per-element step, using either a counter (range) or indexed get.
pub(crate) fn for_each_elem(ctx: &LowerCtx, x: &str, list: Expr, step: Expr, span: Span) -> Expr {
    if let Expr::BuiltinCall { name, args, .. } = &list {
        if matches!(name, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
            let inclusive = matches!(name, Builtin::RangeInclusive);
            return counter_for_in(
                ctx,
                x,
                args[0].clone(),
                args[1].clone(),
                inclusive,
                step,
                span,
            );
        }
    }
    list_for_in(ctx, x, list, step, span)
}

pub(crate) fn empty_list(span: Span) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Var("listOf".into(), span)),
        args: vec![],
        span,
    }
}

pub(crate) fn empty_set(span: Span) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Var("setOf".into(), span)),
        args: vec![],
        span,
    }
}

pub(crate) fn empty_map(span: Span) -> Expr {
    Expr::Call {
        callee: Box::new(Expr::Var("mapOf".into(), span)),
        args: vec![],
        span,
    }
}

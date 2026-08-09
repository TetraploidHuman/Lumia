//! HOF map/filter/fold fusion.

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use super::for_loops::for_each_elem;
use crate::ast::Expr;
use lumia_syntax::Span;

fn peel_hof_maps_filters(
    mut e: &lumia_syntax::Expr,
) -> (
    &lumia_syntax::Expr,
    Vec<&lumia_syntax::Expr>,
    Vec<&lumia_syntax::Expr>,
) {
    let mut maps: Vec<&lumia_syntax::Expr> = Vec::new();
    let mut filters: Vec<&lumia_syntax::Expr> = Vec::new();
    loop {
        match e {
            lumia_syntax::Expr::Pipeline { left, right, .. } => match right.as_ref() {
                lumia_syntax::Expr::Call { callee, args, .. } => match callee.as_ref() {
                    lumia_syntax::Expr::Ident(n, _) if n == "map" && args.len() == 1 => {
                        maps.push(&args[0]);
                        e = left;
                        continue;
                    }
                    lumia_syntax::Expr::Ident(n, _) if n == "filter" && args.len() == 1 => {
                        filters.push(&args[0]);
                        e = left;
                        continue;
                    }
                    _ => break,
                },
                _ => break,
            },
            lumia_syntax::Expr::Call { callee, args, .. } => {
                if let lumia_syntax::Expr::Field { base, field, .. } = callee.as_ref() {
                    if field == "map" && args.len() == 1 {
                        maps.push(&args[0]);
                        e = base;
                        continue;
                    }
                    if field == "filter" && args.len() == 1 {
                        filters.push(&args[0]);
                        e = base;
                        continue;
                    }
                }
                if let lumia_syntax::Expr::Ident(n, _) = callee.as_ref() {
                    if n == "map" && args.len() == 2 {
                        maps.push(&args[1]);
                        e = &args[0];
                        continue;
                    }
                    if n == "filter" && args.len() == 2 {
                        filters.push(&args[1]);
                        e = &args[0];
                        continue;
                    }
                }
                break;
            }
            _ => break,
        }
    }
    maps.reverse();
    filters.reverse();
    (e, maps, filters)
}

fn apply_hof_fn(ctx: &LowerCtx, f: &lumia_syntax::Expr, arg: Expr, span: Span) -> Expr {
    match f {
        lumia_syntax::Expr::Lambda { params, body, .. } if params.len() == 1 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(arg),
            body: Box::new(lower_expr(ctx, body)),
            mutable: false,
        },
        _ => Expr::Call {
            callee: Box::new(lower_expr(ctx, f)),
            args: vec![arg],
            span,
        },
    }
}

fn apply_fold_fn(ctx: &LowerCtx, f: &lumia_syntax::Expr, acc: Expr, x: Expr, span: Span) -> Expr {
    match f {
        lumia_syntax::Expr::Lambda { params, body, .. } if params.len() == 2 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(acc),
            body: Box::new(Expr::Let {
                name: params[1].clone(),
                value: Box::new(x),
                body: Box::new(lower_expr(ctx, body)),
                mutable: false,
            }),
            mutable: false,
        },
        _ => Expr::Call {
            callee: Box::new(lower_expr(ctx, f)),
            args: vec![acc, x],
            span,
        },
    }
}

/// Single-pass `source.map*.filter*.fold` — no intermediate lists.
pub(crate) fn try_fuse_hof_fold(
    ctx: &LowerCtx,
    coll: &lumia_syntax::Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let (source, maps, filters) = peel_hof_maps_filters(coll);
    if maps.is_empty() && filters.is_empty() {
        return None;
    }
    let acc = format!("__fuse_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let mut cur = Expr::Var(x0.clone(), span);
    for m in &maps {
        cur = apply_hof_fn(ctx, m, cur, span);
    }
    let x_mapped = format!("__fuse_xm_{}", span.start.0);
    let mut body = Expr::Assign {
        name: acc.clone(),
        value: Box::new(apply_fold_fn(
            ctx,
            f,
            Expr::Var(acc.clone(), span),
            Expr::Var(x_mapped.clone(), span),
            span,
        )),
        span,
    };
    for p in filters.iter().rev() {
        body = Expr::If {
            cond: Box::new(apply_hof_fn(
                ctx,
                p,
                Expr::Var(x_mapped.clone(), span),
                span,
            )),
            then_branch: Box::new(body),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        };
    }
    let step = Expr::Let {
        name: x_mapped,
        value: Box::new(cur),
        body: Box::new(body),
        mutable: false,
    };
    let source_e = lower_expr(ctx, source);
    Some(Expr::Let {
        name: acc.clone(),
        value: Box::new(lower_expr(ctx, init)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(ctx, &x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    })
}

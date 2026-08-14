//! HOF map/filter/fold fusion.

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use super::for_loops::for_each_elem;
use crate::ast::Expr;
use lumia_syntax::Span;

enum HofStage<'a> {
    Map(&'a lumia_syntax::Expr),
    Filter(&'a lumia_syntax::Expr),
}

/// Peel `source.map*/filter*` into the source and stages in **pipeline order**
/// (first applied to an element first).
fn peel_hof_stages(mut e: &lumia_syntax::Expr) -> (&lumia_syntax::Expr, Vec<HofStage<'_>>) {
    let mut stages: Vec<HofStage<'_>> = Vec::new();
    loop {
        match e {
            lumia_syntax::Expr::Pipeline { left, right, .. } => match right.as_ref() {
                lumia_syntax::Expr::Call { callee, args, .. } => match callee.as_ref() {
                    lumia_syntax::Expr::Ident(n, _) if n == "map" && args.len() == 1 => {
                        stages.push(HofStage::Map(&args[0]));
                        e = left;
                        continue;
                    }
                    lumia_syntax::Expr::Ident(n, _) if n == "filter" && args.len() == 1 => {
                        stages.push(HofStage::Filter(&args[0]));
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
                        stages.push(HofStage::Map(&args[0]));
                        e = base;
                        continue;
                    }
                    if field == "filter" && args.len() == 1 {
                        stages.push(HofStage::Filter(&args[0]));
                        e = base;
                        continue;
                    }
                }
                if let lumia_syntax::Expr::Ident(n, _) = callee.as_ref() {
                    if n == "map" && args.len() == 2 {
                        stages.push(HofStage::Map(&args[1]));
                        e = &args[0];
                        continue;
                    }
                    if n == "filter" && args.len() == 2 {
                        stages.push(HofStage::Filter(&args[1]));
                        e = &args[0];
                        continue;
                    }
                }
                break;
            }
            _ => break,
        }
    }
    // Peeled outermost-first; reverse to source→fold application order.
    stages.reverse();
    (e, stages)
}

fn apply_hof_fn(ctx: &LowerCtx, f: &lumia_syntax::Expr, arg: Expr, span: Span) -> Expr {
    match f {
        lumia_syntax::Expr::Lambda {
            params,
            param_tys,
            body,
            ..
        } if params.len() == 1 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(arg),
            body: Box::new(lower_expr(ctx, body)),
            mutable: false,
            ty: param_tys.first().cloned().flatten(),
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
        lumia_syntax::Expr::Lambda {
            params,
            param_tys,
            body,
            ..
        } if params.len() == 2 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(acc),
            body: Box::new(Expr::Let {
                name: params[1].clone(),
                value: Box::new(x),
                body: Box::new(lower_expr(ctx, body)),
                mutable: false,
                ty: param_tys.get(1).cloned().flatten(),
            }),
            mutable: false,
            ty: param_tys.first().cloned().flatten(),
        },
        _ => Expr::Call {
            callee: Box::new(lower_expr(ctx, f)),
            args: vec![acc, x],
            span,
        },
    }
}

fn and_guard(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(left),
        then_branch: Box::new(right),
        else_branch: Box::new(Expr::Bool(false, span)),
        span,
    }
}

/// Single-pass fused `source.(map|filter)*.fold` — preserves pipeline order.
///
/// `filter(p).map(f)` must test `p(x)` on the **pre-map** element, then fold
/// `f(x)`. The old peel split maps/filters and always filtered the mapped
/// value (wrong for filter-then-map).
pub(crate) fn try_fuse_hof_fold(
    ctx: &LowerCtx,
    coll: &lumia_syntax::Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let (source, stages) = peel_hof_stages(coll);
    if stages.is_empty() {
        return None;
    }
    let acc = format!("__fuse_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);

    let mut cur = Expr::Var(x0.clone(), span);
    let mut guards: Vec<Expr> = Vec::new();
    let mut lets: Vec<(String, Expr)> = Vec::new();
    for (i, stage) in stages.iter().enumerate() {
        match stage {
            HofStage::Filter(p) => {
                guards.push(apply_hof_fn(ctx, p, cur.clone(), span));
            }
            HofStage::Map(m) => {
                let tmp = format!("__fuse_m_{}_{}", span.start.0, i);
                let mapped = apply_hof_fn(ctx, m, cur, span);
                lets.push((tmp.clone(), mapped));
                cur = Expr::Var(tmp, span);
            }
        }
    }
    lets.push((x_out.clone(), cur));

    let mut body = Expr::Assign {
        name: acc.clone(),
        value: Box::new(apply_fold_fn(
            ctx,
            f,
            Expr::Var(acc.clone(), span),
            Expr::Var(x_out, span),
            span,
        )),
        span,
    };
    if let Some(g0) = guards.into_iter().reduce(|a, b| and_guard(a, b, span)) {
        body = Expr::If {
            cond: Box::new(g0),
            then_branch: Box::new(body),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        };
    }

    let mut step = body;
    for (name, value) in lets.into_iter().rev() {
        step = Expr::Let {
            name,
            value: Box::new(value),
            body: Box::new(step),
            mutable: false,
            ty: None,
        };
    }

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
        ty: None,
    })
}

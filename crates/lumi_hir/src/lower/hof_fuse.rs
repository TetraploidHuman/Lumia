//! HOF map/filter/fold fusion.

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use super::for_loops::for_each_elem;
use crate::ast::Expr;
use lumi_syntax::Span;

enum HofStage<'a> {
    Map(&'a lumi_syntax::Expr),
    Filter(&'a lumi_syntax::Expr),
}

/// Peel `source.map*/filter*` into the source and stages in **pipeline order**
/// (first applied to an element first).
fn peel_hof_stages(mut e: &lumi_syntax::Expr) -> (&lumi_syntax::Expr, Vec<HofStage<'_>>) {
    let mut stages: Vec<HofStage<'_>> = Vec::new();
    loop {
        match e {
            lumi_syntax::Expr::Pipeline { left, right, .. } => match right.as_ref() {
                lumi_syntax::Expr::Call { callee, args, .. } => match callee.as_ref() {
                    lumi_syntax::Expr::Ident(n, _) if n == "map" && args.len() == 1 => {
                        stages.push(HofStage::Map(&args[0]));
                        e = left;
                        continue;
                    }
                    lumi_syntax::Expr::Ident(n, _) if n == "filter" && args.len() == 1 => {
                        stages.push(HofStage::Filter(&args[0]));
                        e = left;
                        continue;
                    }
                    _ => break,
                },
                _ => break,
            },
            lumi_syntax::Expr::Call { callee, args, .. } => {
                if let lumi_syntax::Expr::Field { base, field, .. } = callee.as_ref() {
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
                if let lumi_syntax::Expr::Ident(n, _) = callee.as_ref() {
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

fn apply_hof_fn(ctx: &LowerCtx, f: &lumi_syntax::Expr, arg: Expr, span: Span) -> Expr {
    match f {
        lumi_syntax::Expr::Lambda {
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

fn apply_fold_fn(ctx: &LowerCtx, f: &lumi_syntax::Expr, acc: Expr, x: Expr, span: Span) -> Expr {
    match f {
        lumi_syntax::Expr::Lambda {
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
    coll: &lumi_syntax::Expr,
    init: &lumi_syntax::Expr,
    f: &lumi_syntax::Expr,
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

/// Single-pass fused `source.(map|filter)+` → one list build (DESIGN §7.1.1 Fused).
fn try_fuse_hof_build_extend(
    ctx: &LowerCtx,
    base: &lumi_syntax::Expr,
    trailing: HofStage<'_>,
    span: Span,
) -> Option<Expr> {
    let (source, mut stages) = peel_hof_stages(base);
    stages.push(trailing);
    fuse_hof_build_stages(ctx, source, stages, span)
}

pub(crate) fn try_fuse_hof_build_map(
    ctx: &LowerCtx,
    base: &lumi_syntax::Expr,
    f: &lumi_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    try_fuse_hof_build_extend(ctx, base, HofStage::Map(f), span)
}

pub(crate) fn try_fuse_hof_build_filter(
    ctx: &LowerCtx,
    base: &lumi_syntax::Expr,
    f: &lumi_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    try_fuse_hof_build_extend(ctx, base, HofStage::Filter(f), span)
}

fn fuse_hof_build_stages(
    ctx: &LowerCtx,
    source: &lumi_syntax::Expr,
    stages: Vec<HofStage<'_>>,
    span: Span,
) -> Option<Expr> {
    if stages.len() < 2 {
        return None;
    }
    let acc = format!("__fuse_build_{}", span.start.0);
    let x0 = format!("__fuse_bx_{}", span.start.0);
    let x_out = format!("__fuse_bm_{}", span.start.0);

    let mut cur = Expr::Var(x0.clone(), span);
    let mut guards: Vec<Expr> = Vec::new();
    let mut lets: Vec<(String, Expr)> = Vec::new();
    for (i, stage) in stages.iter().enumerate() {
        match stage {
            HofStage::Filter(p) => {
                guards.push(apply_hof_fn(ctx, p, cur.clone(), span));
            }
            HofStage::Map(m) => {
                let tmp = format!("__fuse_bm_{}_{}", span.start.0, i);
                let mapped = apply_hof_fn(ctx, m, cur, span);
                lets.push((tmp.clone(), mapped));
                cur = Expr::Var(tmp, span);
            }
        }
    }
    lets.push((x_out.clone(), cur));

    let append = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: crate::ast::Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x_out, span)],
            span,
        }),
        span,
    };
    let mut body = append;
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
        value: Box::new(crate::lower::empty_list(span)),
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

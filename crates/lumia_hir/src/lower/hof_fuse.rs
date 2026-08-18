//! HOF map/filter/fold fusion.

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use super::for_loops::{empty_list, for_each_elem};
use crate::ast::{Builtin, Expr};
use crate::list_hof::{append_assign, concat_assign};
use lumia_syntax::{BinOp, Span};

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

/// Shared map/filter staging: pipeline order, filter sees pre-map elements.
fn stage_pipeline(
    ctx: &LowerCtx,
    stages: &[HofStage<'_>],
    x0: &str,
    span: Span,
) -> (Expr, Vec<(String, Expr)>, Vec<Expr>) {
    let mut cur = Expr::Var(x0.to_string(), span);
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
    (cur, lets, guards)
}

fn wrap_staged_step(
    mut body: Expr,
    lets: Vec<(String, Expr)>,
    guards: Vec<Expr>,
    span: Span,
) -> Expr {
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
    step
}

/// Single-pass fused `source.(map|filter)*.fold` — preserves pipeline order.
///
/// `filter(p).map(f)` must test `p(x)` on the **pre-map** element, then fold
/// `f(x)`. The old peel split maps/filters and always filtered the mapped
/// value (wrong for filter-then-map).
///
/// Trailing `flatMap` is fused into a nested fold (no concatenated list).
pub(crate) fn try_fuse_hof_fold(
    ctx: &LowerCtx,
    coll: &lumia_syntax::Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, fmap)) = peel_trailing_flat_map(coll) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_fold_flat_map(
            ctx, source, &stages, fmap, init, f, span,
        ));
    }
    let (source, stages) = peel_hof_stages(coll);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_fold(ctx, source, &stages, init, f, span))
}

fn fuse_hof_fold(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let acc = format!("{}_{}", crate::desugar_slots::FUSE_ACC_PREFIX, span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let body = Expr::Assign {
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
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    Expr::Let {
        name: acc.clone(),
        value: Box::new(lower_expr(ctx, init)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

fn fuse_hof_fold_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let acc = format!("{}_{}", crate::desugar_slots::FUSE_ACC_PREFIX, span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);
    let y = format!("__fuse_y_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let bump = Expr::Assign {
        name: acc.clone(),
        value: Box::new(apply_fold_fn(
            ctx,
            f,
            Expr::Var(acc.clone(), span),
            Expr::Var(y.clone(), span),
            span,
        )),
        span,
    };
    let body = for_each_elem(&y, chunk, bump, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    Expr::Let {
        name: acc.clone(),
        value: Box::new(lower_expr(ctx, init)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// Fuse `base.(map|filter)+` then one more `map` into a **single** list builder.
///
/// Requires `base` to already peel to ≥1 map/filter stage (otherwise the normal
/// `lower_list_map` / parallel path applies).
pub(crate) fn try_fuse_hof_build_map(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let (source, mut stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    stages.push(HofStage::Map(f));
    Some(fuse_hof_build(ctx, source, &stages, span))
}

/// Fuse `base.(map|filter)+` then one more `filter` into a single list builder.
pub(crate) fn try_fuse_hof_build_filter(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let (source, mut stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    stages.push(HofStage::Filter(p));
    Some(fuse_hof_build(ctx, source, &stages, span))
}

fn fuse_hof_build(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    span: Span,
) -> Expr {
    // Reuse map-acc prefix so Float ABI keeps this a list builder slot.
    let acc = format!("{}_{}", crate::desugar_slots::MAP_ACC_PREFIX, span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let body = append_assign(&acc, Expr::Var(x_out, span), span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_list(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// Fuse `base.(map|filter)+.flatMap(f)` into one loop: stage then concat chunks.
pub(crate) fn try_fuse_hof_flat_map(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_flat_map(ctx, source, &stages, f, span))
}

fn fuse_hof_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    f: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let acc = format!("{}_{}", crate::desugar_slots::FMAP_ACC_PREFIX, span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
    let body = concat_assign(&acc, chunk, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_list(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// Fuse `base.(map|filter)+.len` into a single count loop (no intermediate List).
///
/// Map callbacks still run (effects preserved); only the list materialization is
/// skipped. Do **not** rewrite map-only chains to `len(source)` — that would drop
/// effectful maps.
///
/// Also fuses a trailing `flatMap` (`….flatMap(f).len()` / `….map….flatMap(f).len()`)
/// into nested count loops without building the concatenated list.
pub(crate) fn try_fuse_hof_len(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, f)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_len_flat_map(ctx, source, &stages, f, span));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_len(ctx, source, &stages, span))
}

fn fuse_hof_len(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    span: Span,
) -> Expr {
    let acc = format!("__len_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let body = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::Binary {
            op: lumia_syntax::BinOp::Add,
            left: Box::new(Expr::Var(acc.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: acc.clone(),
        value: Box::new(Expr::Int(0, span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

fn fuse_hof_len_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    f: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let acc = format!("__len_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);
    let y = format!("__fuse_y_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
    let bump = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::Binary {
            op: lumia_syntax::BinOp::Add,
            left: Box::new(Expr::Var(acc.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    };
    let body = for_each_elem(&y, chunk, bump, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: acc.clone(),
        value: Box::new(Expr::Int(0, span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// Peel a trailing `.flatMap(f)` / `flatMap(xs, f)` / `>> flatMap(f)`.
fn peel_trailing_flat_map(
    e: &lumia_syntax::Expr,
) -> Option<(&lumia_syntax::Expr, &lumia_syntax::Expr)> {
    match e {
        lumia_syntax::Expr::Call { callee, args, .. } => {
            if let lumia_syntax::Expr::Field { base, field, .. } = callee.as_ref() {
                if field == "flatMap" && args.len() == 1 {
                    return Some((base.as_ref(), &args[0]));
                }
            }
            if let lumia_syntax::Expr::Ident(n, _) = callee.as_ref() {
                if n == "flatMap" && args.len() == 2 {
                    return Some((&args[0], &args[1]));
                }
            }
            None
        }
        lumia_syntax::Expr::Pipeline { left, right, .. } => match right.as_ref() {
            lumia_syntax::Expr::Call { callee, args, .. } => match callee.as_ref() {
                lumia_syntax::Expr::Ident(n, _) if n == "flatMap" && args.len() == 1 => {
                    Some((left.as_ref(), &args[0]))
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// Fuse `base.(map|filter)+.isEmpty` — short-circuit without intermediate lists.
///
/// Maps still run (effects); on the first element that survives filters, set
/// false and break. Do **not** rewrite to `source.isEmpty()` (drops effectful maps).
///
/// Trailing `flatMap` is fused the same way (nested scan, no concat list).
pub(crate) fn try_fuse_hof_is_empty(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, f)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_is_empty_flat_map(ctx, source, &stages, f, span));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_is_empty(ctx, source, &stages, span))
}

fn fuse_hof_is_empty(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    span: Span,
) -> Expr {
    let acc = format!("__empty_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let body = Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: acc.clone(),
                value: Box::new(Expr::Bool(false, span)),
                span,
            },
            Expr::Break(span),
        ],
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: acc.clone(),
        value: Box::new(Expr::Bool(true, span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

fn fuse_hof_is_empty_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    f: &lumia_syntax::Expr,
    span: Span,
) -> Expr {
    let acc = format!("__empty_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);
    let y = format!("__fuse_y_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
    let inner = for_each_elem(
        &y,
        chunk,
        Expr::Seq {
            stmts: vec![
                Expr::Assign {
                    name: acc.clone(),
                    value: Box::new(Expr::Bool(false, span)),
                    span,
                },
                Expr::Break(span),
            ],
            span,
        },
        span,
    );
    // Break only exits the inner loop; stop the outer once a chunk yielded.
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(Expr::Binary {
                    op: lumia_syntax::BinOp::Eq,
                    left: Box::new(Expr::Var(acc.clone(), span)),
                    right: Box::new(Expr::Bool(false, span)),
                    span,
                }),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: acc.clone(),
        value: Box::new(Expr::Bool(true, span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// Fuse `base.(map|filter)+.any(p)` — short-circuit without intermediate lists.
pub(crate) fn try_fuse_hof_any(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    try_fuse_hof_search(ctx, base, p, span, FuseSearchKind::Any)
}

/// Fuse `base.(map|filter)+.all(p)`.
pub(crate) fn try_fuse_hof_all(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    try_fuse_hof_search(ctx, base, p, span, FuseSearchKind::All)
}

/// Fuse `base.(map|filter)+.find(p)`.
pub(crate) fn try_fuse_hof_find(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    try_fuse_hof_search(ctx, base, p, span, FuseSearchKind::Find)
}

enum FuseSearchKind {
    Any,
    All,
    Find,
}

fn try_fuse_hof_search(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    span: Span,
    kind: FuseSearchKind,
) -> Option<Expr> {
    if let Some((inner, fmap)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_search_flat_map(
            ctx, source, &stages, fmap, p, span, kind,
        ));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_search(ctx, source, &stages, p, span, kind))
}

fn fuse_hof_search(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    p: &lumia_syntax::Expr,
    span: Span,
    kind: FuseSearchKind,
) -> Expr {
    let (prefix, init) = match kind {
        FuseSearchKind::Any => ("any", Expr::Bool(false, span)),
        FuseSearchKind::All => ("all", Expr::Bool(true, span)),
        FuseSearchKind::Find => ("find", crate::list_hof::option_none(ctx, span)),
    };
    let acc = format!("__{prefix}_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let pred = apply_hof_fn(ctx, p, Expr::Var(x_out.clone(), span), span);
    let body = search_step_body(ctx, kind, &acc, &x_out, pred, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

fn search_step_body(
    ctx: &LowerCtx,
    kind: FuseSearchKind,
    acc: &str,
    x_out: &str,
    pred: Expr,
    span: Span,
) -> Expr {
    match kind {
        FuseSearchKind::Any => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.to_string(),
                        value: Box::new(Expr::Bool(true, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
        FuseSearchKind::All => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Unit(span)),
            else_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.to_string(),
                        value: Box::new(Expr::Bool(false, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            span,
        },
        FuseSearchKind::Find => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.to_string(),
                        value: Box::new(crate::list_hof::option_some(
                            ctx,
                            Expr::Var(x_out.to_string(), span),
                            span,
                        )),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
    }
}

fn fuse_hof_search_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    span: Span,
    kind: FuseSearchKind,
) -> Expr {
    let (prefix, init) = match kind {
        FuseSearchKind::Any => ("any", Expr::Bool(false, span)),
        FuseSearchKind::All => ("all", Expr::Bool(true, span)),
        FuseSearchKind::Find => ("find", crate::list_hof::option_none(ctx, span)),
    };
    let acc = format!("__{prefix}_acc_{}", span.start.0);
    let hit = format!("__{prefix}_hit_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);
    let y = format!("__fuse_y_{}", span.start.0);

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let pred = apply_hof_fn(ctx, p, Expr::Var(y.clone(), span), span);
    let inner_body = match kind {
        FuseSearchKind::Any => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(Expr::Bool(true, span)),
                        span,
                    },
                    Expr::Assign {
                        name: hit.clone(),
                        value: Box::new(Expr::Bool(true, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
        FuseSearchKind::All => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Unit(span)),
            else_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(Expr::Bool(false, span)),
                        span,
                    },
                    Expr::Assign {
                        name: hit.clone(),
                        value: Box::new(Expr::Bool(true, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            span,
        },
        FuseSearchKind::Find => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(crate::list_hof::option_some(
                            ctx,
                            Expr::Var(y.clone(), span),
                            span,
                        )),
                        span,
                    },
                    Expr::Assign {
                        name: hit.clone(),
                        value: Box::new(Expr::Bool(true, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
    };
    let inner = for_each_elem(&y, chunk, inner_body, span);
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(Expr::Var(hit.clone(), span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);

    Expr::Let {
        name: hit.clone(),
        value: Box::new(Expr::Bool(false, span)),
        body: Box::new(Expr::Let {
            name: acc.clone(),
            value: Box::new(init),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    for_each_elem(&x0, source_e, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
            ty: None,
        }),
        mutable: true,
        ty: None,
    }
}

/// Fuse `base.(map|filter)+.get(i)` into a scan that yields the `i`-th survivor
/// (DESIGN §7.3: no materialize when only indexed).
///
/// Out-of-range / negative `i` still traps via empty-list `get` (same as RT).
pub(crate) fn try_fuse_hof_get(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    index: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_get(
        ctx,
        source,
        &stages,
        lower_expr(ctx, index),
        span,
    ))
}

fn fuse_hof_get(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    index: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let seen = format!("__fuse_seen_{uid}");
    // Option slot — must not be `Int(0)` (Float / Task pipelines fail typecheck).
    let acc = format!("__get_acc_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let idx = format!("__fuse_idx_{uid}");

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let on_hit = Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: acc.clone(),
                value: Box::new(crate::list_hof::option_some(
                    ctx,
                    Expr::Var(x_out, span),
                    span,
                )),
                span,
            },
            Expr::Break(span),
        ],
        span,
    };
    let on_miss = Expr::Assign {
        name: seen.clone(),
        value: Box::new(Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Var(seen.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    };
    let body = Expr::If {
        cond: Box::new(Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(Expr::Var(seen.clone(), span)),
            right: Box::new(Expr::Var(idx.clone(), span)),
            span,
        }),
        then_branch: Box::new(on_hit),
        else_branch: Box::new(on_miss),
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    let result = option_payload_or_oob_get(ctx, Expr::Var(acc.clone(), span), span);

    Expr::Let {
        name: idx.clone(),
        value: Box::new(index),
        body: Box::new(Expr::Let {
            name: seen.clone(),
            value: Box::new(Expr::Int(0, span)),
            body: Box::new(Expr::Let {
                name: acc.clone(),
                value: Box::new(crate::list_hof::option_none(ctx, span)),
                body: Box::new(Expr::Seq {
                    stmts: vec![for_each_elem(&x0, source_e, step, span), result],
                    span,
                }),
                mutable: true,
                ty: None,
            }),
            mutable: true,
            ty: None,
        }),
        mutable: false,
        ty: None,
    }
}

/// `Some(v)` → `v`; otherwise empty-list `get` (same OOB trap as RT `ListGet`).
fn option_payload_or_oob_get(ctx: &LowerCtx, acc: Expr, span: Span) -> Expr {
    let some_tag = ctx.lookup_ctor("Some").map(|c| c.tag).unwrap_or(0);
    Expr::If {
        cond: Box::new(Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(Expr::BuiltinCall {
                name: Builtin::AdtTag,
                args: vec![acc.clone()],
                span,
            }),
            right: Box::new(Expr::Int(some_tag, span)),
            span,
        }),
        then_branch: Box::new(Expr::BuiltinCall {
            name: Builtin::AdtField,
            args: vec![
                acc,
                Expr::Int(0, span),
                Expr::String("Some".into(), span),
            ],
            span,
        }),
        else_branch: Box::new(Expr::BuiltinCall {
            name: Builtin::ListGet,
            args: vec![empty_list(span), Expr::Int(0, span)],
            span,
        }),
        span,
    }
}

/// `val ys = pipe.(map|filter)+` when `body` only uses `ys` via `get` / `len` —
/// deforest (DESIGN §7.3 Let-bound case). Returns rewritten `body`.
pub(crate) fn try_deforest_hof_let(
    ctx: &LowerCtx,
    name: &str,
    value: &lumia_syntax::Expr,
    body: &Expr,
    span: Span,
) -> Option<Expr> {
    let (source, stages) = peel_hof_stages(value);
    if stages.is_empty() {
        return None;
    }
    // Lone `.map` must still materialize (par_map / `$Float` clones). DESIGN §7.3
    // is range / map·filter then indexed, or a filter stage.
    if !let_deforest_pipe(source, &stages) {
        return None;
    }
    if !pipe_consumers_only(body, name) {
        return None;
    }
    Some(rewrite_pipe_consumers(ctx, body, name, source, &stages, span))
}

fn let_deforest_pipe(source: &lumia_syntax::Expr, stages: &[HofStage<'_>]) -> bool {
    stages.len() >= 2
        || stages.iter().any(|s| matches!(s, HofStage::Filter(_)))
        || source_is_iota(source)
}

fn source_is_iota(e: &lumia_syntax::Expr) -> bool {
    match e {
        lumia_syntax::Expr::Call { callee, args, .. } if args.len() == 2 => {
            matches!(
                callee.as_ref(),
                lumia_syntax::Expr::Ident(n, _) if n == "range" || n == "rangeInclusive"
            )
        }
        _ => false,
    }
}

fn pipe_consumers_only(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Var(n, _) if n == name => false,
        Expr::BuiltinCall {
            name: b,
            args,
            ..
        } if matches!(b, Builtin::ListGet | Builtin::ListLen)
            && args
                .first()
                .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            args.iter().skip(1).all(|a| !expr_mentions_var(a, name))
        }
        Expr::Let {
            name: bind,
            value,
            body,
            ..
        } => {
            if bind == name {
                !expr_mentions_var(value, name)
            } else {
                pipe_consumers_only(value, name) && pipe_consumers_only(body, name)
            }
        }
        Expr::Assign { value, .. } => pipe_consumers_only(value, name),
        Expr::Seq { stmts, .. } => stmts.iter().all(|s| pipe_consumers_only(s, name)),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            pipe_consumers_only(cond, name)
                && pipe_consumers_only(then_branch, name)
                && pipe_consumers_only(else_branch, name)
        }
        Expr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            pipe_consumers_only(cond, name)
                && pipe_consumers_only(body, name)
                && step
                    .as_ref()
                    .map(|s| pipe_consumers_only(s, name))
                    .unwrap_or(true)
        }
        Expr::Call { callee, args, .. } => {
            pipe_consumers_only(callee, name) && args.iter().all(|a| pipe_consumers_only(a, name))
        }
        Expr::Binary { left, right, .. } => {
            pipe_consumers_only(left, name) && pipe_consumers_only(right, name)
        }
        Expr::Unary { expr, .. } => pipe_consumers_only(expr, name),
        Expr::Lambda { body, .. } => !expr_mentions_var(body, name),
        Expr::Return { value, .. } => pipe_consumers_only(value, name),
        Expr::BuiltinCall { args, .. } => args.iter().all(|a| pipe_consumers_only(a, name)),
        Expr::AdtNew { args, .. } => args.iter().all(|a| pipe_consumers_only(a, name)),
        _ => !expr_mentions_var(e, name),
    }
}

fn expr_mentions_var(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Var(n, _) => n == name,
        Expr::Let {
            name: bind,
            value,
            body,
            ..
        } => {
            expr_mentions_var(value, name) || (bind != name && expr_mentions_var(body, name))
        }
        Expr::Assign { value, .. } => expr_mentions_var(value, name),
        Expr::Seq { stmts, .. } => stmts.iter().any(|s| expr_mentions_var(s, name)),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_mentions_var(cond, name)
                || expr_mentions_var(then_branch, name)
                || expr_mentions_var(else_branch, name)
        }
        Expr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            expr_mentions_var(cond, name)
                || expr_mentions_var(body, name)
                || step.as_ref().is_some_and(|s| expr_mentions_var(s, name))
        }
        Expr::Call { callee, args, .. } => {
            expr_mentions_var(callee, name) || args.iter().any(|a| expr_mentions_var(a, name))
        }
        Expr::Binary { left, right, .. } => {
            expr_mentions_var(left, name) || expr_mentions_var(right, name)
        }
        Expr::Unary { expr, .. } => expr_mentions_var(expr, name),
        Expr::Lambda { body, .. } => expr_mentions_var(body, name),
        Expr::Return { value, .. } => expr_mentions_var(value, name),
        Expr::BuiltinCall { args, .. } => args.iter().any(|a| expr_mentions_var(a, name)),
        Expr::AdtNew { args, .. } => args.iter().any(|a| expr_mentions_var(a, name)),
        _ => false,
    }
}

fn rewrite_pipe_consumers(
    ctx: &LowerCtx,
    e: &Expr,
    name: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    span: Span,
) -> Expr {
    match e {
        Expr::BuiltinCall {
            name: b @ (Builtin::ListGet | Builtin::ListLen),
            args,
            span: s,
        } if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            match b {
                Builtin::ListGet => {
                    let idx = args.get(1).cloned().unwrap_or(Expr::Int(0, *s));
                    fuse_hof_get(ctx, source, stages, idx, *s)
                }
                Builtin::ListLen => fuse_hof_len(ctx, source, stages, *s),
                _ => unreachable!(),
            }
        }
        Expr::Let {
            name: bind,
            value,
            body,
            mutable,
            ty,
        } => {
            if bind == name {
                Expr::Let {
                    name: bind.clone(),
                    value: Box::new(rewrite_pipe_consumers(
                        ctx, value, name, source, stages, span,
                    )),
                    body: Box::new((**body).clone()),
                    mutable: *mutable,
                    ty: ty.clone(),
                }
            } else {
                Expr::Let {
                    name: bind.clone(),
                    value: Box::new(rewrite_pipe_consumers(
                        ctx, value, name, source, stages, span,
                    )),
                    body: Box::new(rewrite_pipe_consumers(
                        ctx, body, name, source, stages, span,
                    )),
                    mutable: *mutable,
                    ty: ty.clone(),
                }
            }
        }
        Expr::Assign {
            name: n,
            value,
            span: s,
        } => Expr::Assign {
            name: n.clone(),
            value: Box::new(rewrite_pipe_consumers(
                ctx, value, name, source, stages, span,
            )),
            span: *s,
        },
        Expr::Seq { stmts, span: s } => Expr::Seq {
            stmts: stmts
                .iter()
                .map(|st| rewrite_pipe_consumers(ctx, st, name, source, stages, span))
                .collect(),
            span: *s,
        },
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span: s,
        } => Expr::If {
            cond: Box::new(rewrite_pipe_consumers(
                ctx, cond, name, source, stages, span,
            )),
            then_branch: Box::new(rewrite_pipe_consumers(
                ctx, then_branch, name, source, stages, span,
            )),
            else_branch: Box::new(rewrite_pipe_consumers(
                ctx, else_branch, name, source, stages, span,
            )),
            span: *s,
        },
        Expr::Loop {
            cond,
            body,
            step,
            span: s,
        } => Expr::Loop {
            cond: Box::new(rewrite_pipe_consumers(
                ctx, cond, name, source, stages, span,
            )),
            body: Box::new(rewrite_pipe_consumers(
                ctx, body, name, source, stages, span,
            )),
            step: step.as_ref().map(|st| {
                Box::new(rewrite_pipe_consumers(
                    ctx, st, name, source, stages, span,
                ))
            }),
            span: *s,
        },
        Expr::Call {
            callee,
            args,
            span: s,
        } => Expr::Call {
            callee: Box::new(rewrite_pipe_consumers(
                ctx, callee, name, source, stages, span,
            )),
            args: args
                .iter()
                .map(|a| rewrite_pipe_consumers(ctx, a, name, source, stages, span))
                .collect(),
            span: *s,
        },
        Expr::Binary {
            op,
            left,
            right,
            span: s,
        } => Expr::Binary {
            op: *op,
            left: Box::new(rewrite_pipe_consumers(
                ctx, left, name, source, stages, span,
            )),
            right: Box::new(rewrite_pipe_consumers(
                ctx, right, name, source, stages, span,
            )),
            span: *s,
        },
        Expr::Unary { op, expr, span: s } => Expr::Unary {
            op: *op,
            expr: Box::new(rewrite_pipe_consumers(
                ctx, expr, name, source, stages, span,
            )),
            span: *s,
        },
        Expr::Return { value, span: s } => Expr::Return {
            value: Box::new(rewrite_pipe_consumers(
                ctx, value, name, source, stages, span,
            )),
            span: *s,
        },
        Expr::BuiltinCall {
            name: b,
            args,
            span: s,
        } => Expr::BuiltinCall {
            name: *b,
            args: args
                .iter()
                .map(|a| rewrite_pipe_consumers(ctx, a, name, source, stages, span))
                .collect(),
            span: *s,
        },
        Expr::AdtNew {
            adt_name,
            variant,
            tag,
            args,
            span: s,
        } => Expr::AdtNew {
            adt_name: adt_name.clone(),
            variant: variant.clone(),
            tag: *tag,
            args: args
                .iter()
                .map(|a| rewrite_pipe_consumers(ctx, a, name, source, stages, span))
                .collect(),
            span: *s,
        },
        other => other.clone(),
    }
}

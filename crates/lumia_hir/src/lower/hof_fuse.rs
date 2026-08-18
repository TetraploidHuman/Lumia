//! HOF map/filter/fold fusion.

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use super::for_loops::{empty_list, empty_map, empty_set, for_each_elem};
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
    if let Some((inner, cut)) = peel_trailing_take_drop(coll) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            TrailingCut::Take(_) => fuse_hof_fold_under_take(ctx, inner, n, init, f, span),
            TrailingCut::Drop(_) => fuse_hof_fold_under_drop(ctx, inner, n, init, f, span),
        };
    }
    fuse_hof_fold_on(ctx, coll, init, f, span)
}

fn fuse_hof_fold_on(
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

fn fuse_hof_fold_under_take(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    if let Expr::Int(k, _) = n {
        if k <= 0 {
            return Some(lower_expr(ctx, init));
        }
    }
    let uid = span.start.0;
    let raw = format!("__take_raw_{uid}");
    let lim = format!("__take_n_{uid}");
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(fm) => fuse_hof_fold_flat_map_capped(ctx, source, &stages, fm, init, f, &lim, span),
        None => fuse_hof_fold_capped(ctx, source, &stages, init, f, &lim, span),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
}

fn fuse_hof_fold_under_drop(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    if matches!(n, Expr::Int(0, _)) {
        return fuse_hof_fold_on(ctx, inner, init, f, span);
    }
    let uid = span.start.0;
    let raw = format!("__drop_raw_{uid}");
    let lim = format!("__drop_n_{uid}");
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(fm) => fuse_hof_fold_flat_map_skip(ctx, source, &stages, fm, init, f, &lim, span),
        None => fuse_hof_fold_skip(ctx, source, &stages, init, f, &lim, span),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
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

fn fuse_hof_fold_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::FUSE_ACC_PREFIX, uid);
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let bump = Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: acc.clone(),
                value: Box::new(apply_fold_fn(
                    ctx,
                    f,
                    Expr::Var(acc.clone(), span),
                    Expr::Var(x_out, span),
                    span,
                )),
                span,
            },
            Expr::Assign {
                name: k.clone(),
                value: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    left: Box::new(Expr::Var(k.clone(), span)),
                    right: Box::new(Expr::Int(1, span)),
                    span,
                }),
                span,
            },
            Expr::If {
                cond: Box::new(take_reached(&k, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner = wrap_staged_step(bump, lets, guards, span);
    let step = take_limit_step(&k, lim, inner, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (k, Expr::Int(0, span), true),
            (acc.clone(), lower_expr(ctx, init), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_fold_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::FUSE_ACC_PREFIX, uid);
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let bump = Expr::Assign {
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
    let body = drop_then(&skipped, lim, bump, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (acc.clone(), lower_expr(ctx, init), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_fold_flat_map_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::FUSE_ACC_PREFIX, uid);
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let bump = Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: acc.clone(),
                value: Box::new(apply_fold_fn(
                    ctx,
                    f,
                    Expr::Var(acc.clone(), span),
                    Expr::Var(y.clone(), span),
                    span,
                )),
                span,
            },
            Expr::Assign {
                name: k.clone(),
                value: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    left: Box::new(Expr::Var(k.clone(), span)),
                    right: Box::new(Expr::Int(1, span)),
                    span,
                }),
                span,
            },
            Expr::If {
                cond: Box::new(take_reached(&k, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner = for_each_elem(&y, chunk, bump, span);
    let step = take_limit_step(&k, lim, wrap_staged_step(inner, lets, guards, span), span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (k, Expr::Int(0, span), true),
            (acc.clone(), lower_expr(ctx, init), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_fold_flat_map_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    init: &lumia_syntax::Expr,
    f: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::FUSE_ACC_PREFIX, uid);
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
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
    let inner_y = drop_then(&skipped, lim, bump, span);
    let body = for_each_elem(&y, chunk, inner_y, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (acc.clone(), lower_expr(ctx, init), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
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
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            TrailingCut::Take(_) => fuse_hof_len_under_take(ctx, inner, n, span),
            TrailingCut::Drop(_) => fuse_hof_len_under_drop(ctx, inner, n, span),
        };
    }
    fuse_hof_len_on(ctx, base, span)
}

fn fuse_hof_len_on(ctx: &LowerCtx, base: &lumia_syntax::Expr, span: Span) -> Option<Expr> {
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

fn fuse_hof_len_under_take(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    span: Span,
) -> Option<Expr> {
    let uid = span.start.0;
    let raw = format!("__take_raw_{uid}");
    let lim = format!("__take_n_{uid}");
    // Nested take/drop: `min(lim, len(inner))` (nested cuts are rare; keep correct).
    if peel_trailing_take_drop(inner).is_some() {
        let inner_len = try_fuse_hof_len(ctx, inner, span)?;
        let body = Expr::If {
            cond: Box::new(Expr::Binary {
                op: BinOp::Lt,
                left: Box::new(Expr::Var(lim.clone(), span)),
                right: Box::new(inner_len.clone()),
                span,
            }),
            then_branch: Box::new(Expr::Var(lim.clone(), span)),
            else_branch: Box::new(inner_len),
            span,
        };
        return Some(bind_nonneg_lim(raw, lim, n, body, span));
    }
    // Cap count at lim (early stop) — same as counting a fused take.
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(f) => fuse_hof_len_flat_map_capped(ctx, source, &stages, f, &lim, span),
        None => fuse_hof_len_capped(ctx, source, &stages, &lim, span),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
}

fn fuse_hof_len_under_drop(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    span: Span,
) -> Option<Expr> {
    let uid = span.start.0;
    let raw = format!("__drop_raw_{uid}");
    let lim = format!("__drop_n_{uid}");
    // Nested take/drop: `max(0, len(inner) - lim)`.
    if peel_trailing_take_drop(inner).is_some() {
        let inner_len = try_fuse_hof_len(ctx, inner, span)?;
        let body = Expr::If {
            cond: Box::new(Expr::Binary {
                op: BinOp::Ge,
                left: Box::new(Expr::Var(lim.clone(), span)),
                right: Box::new(inner_len.clone()),
                span,
            }),
            then_branch: Box::new(Expr::Int(0, span)),
            else_branch: Box::new(Expr::Binary {
                op: BinOp::Sub,
                left: Box::new(inner_len),
                right: Box::new(Expr::Var(lim.clone(), span)),
                span,
            }),
            span,
        };
        return Some(bind_nonneg_lim(raw, lim, n, body, span));
    }
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(f) => fuse_hof_len_flat_map_skip(ctx, source, &stages, f, &lim, span),
        None => fuse_hof_len_skip(ctx, source, &stages, &lim, span),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
}

fn peel_len_base<'a>(
    base: &'a lumia_syntax::Expr,
) -> Option<(
    &'a lumia_syntax::Expr,
    Vec<HofStage<'a>>,
    Option<&'a lumia_syntax::Expr>,
)> {
    if let Some((inner, f)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some((source, stages, Some(f)));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some((source, stages, None))
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

fn len_bump(acc: &str, span: Span) -> Expr {
    Expr::Assign {
        name: acc.to_string(),
        value: Box::new(Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Var(acc.to_string(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    }
}

/// Count survivors but stop once `acc >= lim` (fused `.take(n).len()`).
fn fuse_hof_len_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    lim: &str,
    span: Span,
) -> Expr {
    let acc = format!("__len_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let bump = Expr::Seq {
        stmts: vec![
            len_bump(&acc, span),
            Expr::If {
                cond: Box::new(take_reached(&acc, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner = wrap_staged_step(bump, lets, guards, span);
    let step = take_limit_step(&acc, lim, inner, span);
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

fn fuse_hof_len_flat_map_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    f: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let acc = format!("__len_acc_{}", span.start.0);
    let x0 = format!("__fuse_x_{}", span.start.0);
    let x_out = format!("__fuse_xm_{}", span.start.0);
    let y = format!("__fuse_y_{}", span.start.0);
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
    let bump = Expr::Seq {
        stmts: vec![
            len_bump(&acc, span),
            Expr::If {
                cond: Box::new(take_reached(&acc, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner_y = take_limit_step(&acc, lim, bump, span);
    let body = Expr::Seq {
        stmts: vec![
            for_each_elem(&y, chunk, inner_y, span),
            Expr::If {
                cond: Box::new(take_reached(&acc, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner = wrap_staged_step(body, lets, guards, span);
    let step = take_limit_step(&acc, lim, inner, span);
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

/// Skip first `lim` survivors, then count the rest (fused `.drop(n).len()`).
fn fuse_hof_len_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("__len_acc_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let body = drop_then(&skipped, lim, len_bump(&acc, span), span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (acc.clone(), Expr::Int(0, span), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_len_flat_map_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    f: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("__len_acc_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
    let inner_y = drop_then(&skipped, lim, len_bump(&acc, span), span);
    let body = for_each_elem(&y, chunk, inner_y, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (acc.clone(), Expr::Int(0, span), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn drop_then(skipped: &str, lim: &str, then_branch: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(Expr::Binary {
            op: BinOp::Lt,
            left: Box::new(Expr::Var(skipped.to_string(), span)),
            right: Box::new(Expr::Var(lim.to_string(), span)),
            span,
        }),
        then_branch: Box::new(Expr::Assign {
            name: skipped.to_string(),
            value: Box::new(Expr::Binary {
                op: BinOp::Add,
                left: Box::new(Expr::Var(skipped.to_string(), span)),
                right: Box::new(Expr::Int(1, span)),
                span,
            }),
            span,
        }),
        else_branch: Box::new(then_branch),
        span,
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

/// Peel trailing `.toMap()` / `toMap(xs)` / `>> toMap`.
fn peel_trailing_to_map(e: &lumia_syntax::Expr) -> Option<&lumia_syntax::Expr> {
    peel_trailing_nullary(e, "toMap")
}

/// Peel trailing `.toSet()` / `toSet(xs)` / `>> toSet`.
fn peel_trailing_to_set(e: &lumia_syntax::Expr) -> Option<&lumia_syntax::Expr> {
    peel_trailing_nullary(e, "toSet")
}

fn peel_trailing_nullary<'a>(
    e: &'a lumia_syntax::Expr,
    method: &str,
) -> Option<&'a lumia_syntax::Expr> {
    match e {
        lumia_syntax::Expr::Call { callee, args, .. } => {
            if let lumia_syntax::Expr::Field { base, field, .. } = callee.as_ref() {
                if field == method && args.is_empty() {
                    return Some(base.as_ref());
                }
            }
            if let lumia_syntax::Expr::Ident(n, _) = callee.as_ref() {
                if n == method && args.len() == 1 {
                    return Some(&args[0]);
                }
            }
            None
        }
        lumia_syntax::Expr::Pipeline { left, right, .. } => match right.as_ref() {
            lumia_syntax::Expr::Call { callee, args, .. } if args.is_empty() => {
                match callee.as_ref() {
                    lumia_syntax::Expr::Ident(n, _) if n == method => Some(left.as_ref()),
                    _ => None,
                }
            }
            lumia_syntax::Expr::Ident(n, _) if n == method => Some(left.as_ref()),
            _ => None,
        },
        _ => None,
    }
}

enum TrailingCut<'a> {
    Take(&'a lumia_syntax::Expr),
    /// `.drop(n)` / `.slice(n)` / `slice(xs, n)` — drop first `n` survivors.
    Drop(&'a lumia_syntax::Expr),
}

/// Peel trailing `.take(n)` / `.drop(n)` / `.slice(n)` (and free / `>>` forms).
fn peel_trailing_take_drop(
    e: &lumia_syntax::Expr,
) -> Option<(&lumia_syntax::Expr, TrailingCut<'_>)> {
    match e {
        lumia_syntax::Expr::Call { callee, args, .. } => {
            if let lumia_syntax::Expr::Field { base, field, .. } = callee.as_ref() {
                if args.len() == 1 {
                    if field == "take" {
                        return Some((base.as_ref(), TrailingCut::Take(&args[0])));
                    }
                    if field == "drop" || field == "slice" {
                        return Some((base.as_ref(), TrailingCut::Drop(&args[0])));
                    }
                }
            }
            if let lumia_syntax::Expr::Ident(n, _) = callee.as_ref() {
                if args.len() == 2 {
                    if n == "take" {
                        return Some((&args[0], TrailingCut::Take(&args[1])));
                    }
                    if n == "drop" || n == "slice" {
                        return Some((&args[0], TrailingCut::Drop(&args[1])));
                    }
                }
            }
            None
        }
        lumia_syntax::Expr::Pipeline { left, right, .. } => match right.as_ref() {
            lumia_syntax::Expr::Call { callee, args, .. } => match callee.as_ref() {
                lumia_syntax::Expr::Ident(n, _) if args.len() == 1 => {
                    if n == "take" {
                        Some((left.as_ref(), TrailingCut::Take(&args[0])))
                    } else if n == "drop" || n == "slice" {
                        Some((left.as_ref(), TrailingCut::Drop(&args[0])))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

fn bind_nonneg_lim(raw: String, lim: String, n: Expr, body: Expr, span: Span) -> Expr {
    match n {
        Expr::Int(k, s) if k >= 0 => {
            let _ = raw;
            Expr::Let {
                name: lim,
                value: Box::new(Expr::Int(k, s)),
                body: Box::new(body),
                mutable: false,
                ty: None,
            }
        }
        Expr::Int(_, s) => {
            let _ = raw;
            Expr::Let {
                name: lim,
                value: Box::new(Expr::Int(0, s)),
                body: Box::new(body),
                mutable: false,
                ty: None,
            }
        }
        n => Expr::Let {
            name: raw.clone(),
            value: Box::new(n),
            body: Box::new(Expr::Let {
                name: lim,
                value: Box::new(Expr::If {
                    cond: Box::new(Expr::Binary {
                        op: BinOp::Lt,
                        left: Box::new(Expr::Var(raw.clone(), span)),
                        right: Box::new(Expr::Int(0, span)),
                        span,
                    }),
                    then_branch: Box::new(Expr::Int(0, span)),
                    else_branch: Box::new(Expr::Var(raw, span)),
                    span,
                }),
                body: Box::new(body),
                mutable: false,
                ty: None,
            }),
            mutable: false,
            ty: None,
        },
    }
}

fn empty_get_oob(span: Span) -> Expr {
    Expr::BuiltinCall {
        name: Builtin::ListGet,
        args: vec![empty_list(span), Expr::Int(0, span)],
        span,
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
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            // take(0).isEmpty → true; take(k>0) ≡ inner.isEmpty.
            TrailingCut::Take(_) => {
                if let Expr::Int(k, _) = n {
                    if k <= 0 {
                        return Some(Expr::Bool(true, span));
                    }
                    return try_fuse_hof_is_empty(ctx, inner, span).or_else(|| {
                        Some(Expr::Binary {
                            op: BinOp::Eq,
                            left: Box::new(Expr::BuiltinCall {
                                name: Builtin::ListLen,
                                args: vec![lower_expr(ctx, inner)],
                                span,
                            }),
                            right: Box::new(Expr::Int(0, span)),
                            span,
                        })
                    });
                }
                let inner_e =
                    try_fuse_hof_is_empty(ctx, inner, span).unwrap_or_else(|| Expr::Binary {
                        op: BinOp::Eq,
                        left: Box::new(Expr::BuiltinCall {
                            name: Builtin::ListLen,
                            args: vec![lower_expr(ctx, inner)],
                            span,
                        }),
                        right: Box::new(Expr::Int(0, span)),
                        span,
                    });
                let uid = span.start.0;
                let raw = format!("__take_raw_{uid}");
                let lim = format!("__take_n_{uid}");
                Some(bind_nonneg_lim(
                    raw,
                    lim.clone(),
                    n,
                    Expr::If {
                        cond: Box::new(Expr::Binary {
                            op: BinOp::Eq,
                            left: Box::new(Expr::Var(lim, span)),
                            right: Box::new(Expr::Int(0, span)),
                            span,
                        }),
                        then_branch: Box::new(Expr::Bool(true, span)),
                        else_branch: Box::new(inner_e),
                        span,
                    },
                    span,
                ))
            }
            // drop(0).isEmpty ≡ inner.isEmpty; else count-after-skip == 0.
            TrailingCut::Drop(_) => {
                if matches!(n, Expr::Int(0, _)) {
                    return try_fuse_hof_is_empty(ctx, inner, span);
                }
                fuse_hof_len_under_drop(ctx, inner, n, span).map(|len_e| Expr::Binary {
                    op: BinOp::Eq,
                    left: Box::new(len_e),
                    right: Box::new(Expr::Int(0, span)),
                    span,
                })
            }
        };
    }
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

/// Fuse `base.(map|filter)+.contains(x)` as a short-circuit equality scan.
///
/// Needle is bound once (effects run once). Map/Set `.contains` is untouched
/// (no map/filter stages → `None`).
pub(crate) fn try_fuse_hof_contains(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    needle: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    if let Some(inner) = peel_trailing_to_map(base) {
        return fuse_tomap_lookup(
            ctx,
            inner,
            lower_expr(ctx, needle),
            span,
            TomapLookup::Contains,
        );
    }
    if let Some(inner) = peel_trailing_to_set(base) {
        return fuse_toset_contains(ctx, inner, lower_expr(ctx, needle), span);
    }
    let nv = format!("__contains_n_{}", span.start.0);
    let p = contains_eq_lambda(&nv, span);
    let fused = try_fuse_hof_search(ctx, base, &p, span, FuseSearchKind::Any)?;
    Some(Expr::Let {
        name: nv,
        value: Box::new(lower_expr(ctx, needle)),
        body: Box::new(fused),
        mutable: false,
        ty: None,
    })
}

/// `pipe.toSet().contains(x)` — membership scan; empty map/filter still scans the source.
fn fuse_toset_contains(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    needle: Expr,
    span: Span,
) -> Option<Expr> {
    let nv = format!("__contains_n_{}", span.start.0);
    let p = contains_eq_lambda(&nv, span);
    let fused = match try_fuse_hof_search(ctx, inner, &p, span, FuseSearchKind::Any) {
        Some(e) => e,
        None => {
            let (source, stages) = peel_hof_stages(inner);
            fuse_hof_search(ctx, source, &stages, &p, span, FuseSearchKind::Any)
        }
    };
    Some(Expr::Let {
        name: nv,
        value: Box::new(needle),
        body: Box::new(fused),
        mutable: false,
        ty: None,
    })
}

fn contains_eq_lambda(needle_name: &str, span: Span) -> lumia_syntax::Expr {
    let x = format!("__contains_x_{}", span.start.0);
    lumia_syntax::Expr::Lambda {
        params: vec![x.clone()],
        param_tys: vec![None],
        bare_it: false,
        body: Box::new(lumia_syntax::Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(lumia_syntax::Expr::Ident(x, span)),
            right: Box::new(lumia_syntax::Expr::Ident(needle_name.to_string(), span)),
            span,
        }),
        span,
    }
}

#[derive(Clone, Copy)]
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
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            TrailingCut::Take(_) => fuse_hof_search_under_take(ctx, inner, n, p, span, kind),
            TrailingCut::Drop(_) => fuse_hof_search_under_drop(ctx, inner, n, p, span, kind),
        };
    }
    fuse_hof_search_on(ctx, base, p, span, kind)
}

fn fuse_hof_search_on(
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

fn search_vacuous(ctx: &LowerCtx, kind: FuseSearchKind, span: Span) -> Expr {
    match kind {
        FuseSearchKind::Any => Expr::Bool(false, span),
        FuseSearchKind::All => Expr::Bool(true, span),
        FuseSearchKind::Find => crate::list_hof::option_none(ctx, span),
    }
}

fn fuse_hof_search_under_take(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    p: &lumia_syntax::Expr,
    span: Span,
    kind: FuseSearchKind,
) -> Option<Expr> {
    if let Expr::Int(k, _) = n {
        if k <= 0 {
            return Some(search_vacuous(ctx, kind, span));
        }
    }
    let uid = span.start.0;
    let raw = format!("__take_raw_{uid}");
    let lim = format!("__take_n_{uid}");
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(fm) => fuse_hof_search_flat_map_capped(ctx, source, &stages, fm, p, &lim, span, kind),
        None => fuse_hof_search_capped(ctx, source, &stages, p, &lim, span, kind),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
}

fn fuse_hof_search_under_drop(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    p: &lumia_syntax::Expr,
    span: Span,
    kind: FuseSearchKind,
) -> Option<Expr> {
    if matches!(n, Expr::Int(0, _)) {
        return fuse_hof_search_on(ctx, inner, p, span, kind);
    }
    let uid = span.start.0;
    let raw = format!("__drop_raw_{uid}");
    let lim = format!("__drop_n_{uid}");
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(fm) => fuse_hof_search_flat_map_skip(ctx, source, &stages, fm, p, &lim, span, kind),
        None => fuse_hof_search_skip(ctx, source, &stages, p, &lim, span, kind),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
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

fn search_short_circuit(ctx: &LowerCtx, kind: FuseSearchKind, acc: &str, span: Span) -> Expr {
    match kind {
        FuseSearchKind::Any => Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(Expr::Var(acc.to_string(), span)),
            right: Box::new(Expr::Bool(true, span)),
            span,
        },
        FuseSearchKind::All => Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(Expr::Var(acc.to_string(), span)),
            right: Box::new(Expr::Bool(false, span)),
            span,
        },
        FuseSearchKind::Find => is_some_var(ctx, acc, span),
    }
}

fn fuse_hof_search_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    p: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
    kind: FuseSearchKind,
) -> Expr {
    let (prefix, init) = match kind {
        FuseSearchKind::Any => ("any", Expr::Bool(false, span)),
        FuseSearchKind::All => ("all", Expr::Bool(true, span)),
        FuseSearchKind::Find => ("find", crate::list_hof::option_none(ctx, span)),
    };
    let uid = span.start.0;
    let acc = format!("__{prefix}_acc_{uid}");
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let pred = apply_hof_fn(ctx, p, Expr::Var(x_out.clone(), span), span);
    let body = Expr::Seq {
        stmts: vec![
            search_step_body(ctx, kind, &acc, &x_out, pred, span),
            Expr::Assign {
                name: k.clone(),
                value: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    left: Box::new(Expr::Var(k.clone(), span)),
                    right: Box::new(Expr::Int(1, span)),
                    span,
                }),
                span,
            },
            Expr::If {
                cond: Box::new(take_reached(&k, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner = wrap_staged_step(body, lets, guards, span);
    let step = take_limit_step(&k, lim, inner, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![(k, Expr::Int(0, span), true), (acc.clone(), init, true)],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_search_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    p: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
    kind: FuseSearchKind,
) -> Expr {
    let (prefix, init) = match kind {
        FuseSearchKind::Any => ("any", Expr::Bool(false, span)),
        FuseSearchKind::All => ("all", Expr::Bool(true, span)),
        FuseSearchKind::Find => ("find", crate::list_hof::option_none(ctx, span)),
    };
    let uid = span.start.0;
    let acc = format!("__{prefix}_acc_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let pred = apply_hof_fn(ctx, p, Expr::Var(x_out.clone(), span), span);
    let body = drop_then(
        &skipped,
        lim,
        search_step_body(ctx, kind, &acc, &x_out, pred, span),
        span,
    );
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (acc.clone(), init, true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_search_flat_map_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
    kind: FuseSearchKind,
) -> Expr {
    let (prefix, init) = match kind {
        FuseSearchKind::Any => ("any", Expr::Bool(false, span)),
        FuseSearchKind::All => ("all", Expr::Bool(true, span)),
        FuseSearchKind::Find => ("find", crate::list_hof::option_none(ctx, span)),
    };
    let uid = span.start.0;
    let acc = format!("__{prefix}_acc_{uid}");
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let pred = apply_hof_fn(ctx, p, Expr::Var(y.clone(), span), span);
    let inner_y = Expr::Seq {
        stmts: vec![
            search_step_body(ctx, kind, &acc, &y, pred, span),
            Expr::Assign {
                name: k.clone(),
                value: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    left: Box::new(Expr::Var(k.clone(), span)),
                    right: Box::new(Expr::Int(1, span)),
                    span,
                }),
                span,
            },
            Expr::If {
                cond: Box::new(take_reached(&k, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner = for_each_elem(&y, chunk, inner_y, span);
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(search_short_circuit(ctx, kind, &acc, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::If {
                    cond: Box::new(take_reached(&k, lim, span)),
                    then_branch: Box::new(Expr::Break(span)),
                    else_branch: Box::new(Expr::Unit(span)),
                    span,
                }),
                span,
            },
        ],
        span,
    };
    let step = take_limit_step(&k, lim, wrap_staged_step(body, lets, guards, span), span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![(k, Expr::Int(0, span), true), (acc.clone(), init, true)],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_search_flat_map_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    p: &lumia_syntax::Expr,
    lim: &str,
    span: Span,
    kind: FuseSearchKind,
) -> Expr {
    let (prefix, init) = match kind {
        FuseSearchKind::Any => ("any", Expr::Bool(false, span)),
        FuseSearchKind::All => ("all", Expr::Bool(true, span)),
        FuseSearchKind::Find => ("find", crate::list_hof::option_none(ctx, span)),
    };
    let uid = span.start.0;
    let acc = format!("__{prefix}_acc_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let pred = apply_hof_fn(ctx, p, Expr::Var(y.clone(), span), span);
    let inner_y = drop_then(
        &skipped,
        lim,
        search_step_body(ctx, kind, &acc, &y, pred, span),
        span,
    );
    let inner = for_each_elem(&y, chunk, inner_y, span);
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(search_short_circuit(ctx, kind, &acc, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (acc.clone(), init, true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
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
    if let Some(inner) = peel_trailing_to_map(base) {
        return fuse_tomap_lookup(ctx, inner, lower_expr(ctx, index), span, TomapLookup::Get);
    }
    fuse_hof_get_resolved(ctx, base, lower_expr(ctx, index), span)
}

/// Peel nested `.take` / `.drop` then fuse get (so `drop.take.get` / `take.drop.get` work).
fn fuse_hof_get_resolved(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    index: Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            TrailingCut::Take(_) => fuse_hof_get_under_take(ctx, inner, n, index, span),
            TrailingCut::Drop(_) => fuse_hof_get_under_drop(ctx, inner, n, index, span),
        };
    }
    fuse_hof_get_on(ctx, base, index, span)
}

fn fuse_hof_get_on(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    index: Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, fmap)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_get_flat_map(
            ctx, source, &stages, fmap, index, span,
        ));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_get(ctx, source, &stages, index, span))
}

fn fuse_hof_get_under_take(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    index: Expr,
    span: Span,
) -> Option<Expr> {
    let uid = span.start.0;
    let raw = format!("__take_raw_{uid}");
    let lim = format!("__take_n_{uid}");
    let idx = format!("__take_get_idx_{uid}");
    let fused = fuse_hof_get_resolved(ctx, inner, Expr::Var(idx.clone(), span), span)?;
    let body = Expr::If {
        cond: Box::new(Expr::Binary {
            op: BinOp::Lt,
            left: Box::new(Expr::Var(idx.clone(), span)),
            right: Box::new(Expr::Var(lim.clone(), span)),
            span,
        }),
        then_branch: Box::new(fused),
        else_branch: Box::new(empty_get_oob(span)),
        span,
    };
    Some(bind_nonneg_lim(
        raw,
        lim,
        n,
        Expr::Let {
            name: idx,
            value: Box::new(index),
            body: Box::new(body),
            mutable: false,
            ty: None,
        },
        span,
    ))
}

fn fuse_hof_get_under_drop(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    n: Expr,
    index: Expr,
    span: Span,
) -> Option<Expr> {
    let uid = span.start.0;
    let raw = format!("__drop_raw_{uid}");
    let lim = format!("__drop_n_{uid}");
    let adj = Expr::Binary {
        op: BinOp::Add,
        left: Box::new(index),
        right: Box::new(Expr::Var(lim.clone(), span)),
        span,
    };
    let fused = fuse_hof_get_resolved(ctx, inner, adj, span)?;
    Some(bind_nonneg_lim(raw, lim, n, fused, span))
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

    let body = get_scan_step(ctx, &seen, &acc, &idx, &x_out, span);
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

fn fuse_hof_get_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    index: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let seen = format!("__fuse_seen_{uid}");
    let acc = format!("__get_acc_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let idx = format!("__fuse_idx_{uid}");

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let inner = for_each_elem(
        &y,
        chunk,
        get_scan_step(ctx, &seen, &acc, &idx, &y, span),
        span,
    );
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(is_some_var(ctx, &acc, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    let result = option_payload_or_oob_get(ctx, Expr::Var(acc.clone(), span), span);

    nest_lets(
        vec![
            (idx, index, false),
            (seen, Expr::Int(0, span), true),
            (acc, crate::list_hof::option_none(ctx, span), true),
        ],
        Expr::Seq {
            stmts: vec![for_each_elem(&x0, source_e, step, span), result],
            span,
        },
    )
}

/// Fuse `base.(map|filter)+.take(n)` — fill a builder and stop after `n` survivors
/// (negative/`0` → empty; matches RT `lumia_list_take`).
///
/// Also peels trailing `.drop` / nested `.take` on `base` (`drop.take` / `take.take`).
pub(crate) fn try_fuse_hof_take(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    n: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let n = lower_expr(ctx, n);
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let m = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            // `take(m).take(n)` ≡ `take(min(m,n))`.
            TrailingCut::Take(_) => Some(fuse_hof_take_min(ctx, inner, m, n, span)),
            // `drop(m).take(n)` — skip `m` then take `n`.
            TrailingCut::Drop(_) => Some(fuse_hof_take_after_drop(ctx, inner, m, n, span)),
        };
    }
    fuse_hof_take_on(ctx, base, n, span)
}

fn fuse_hof_take_on(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    n: Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, fmap)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_take_flat_map(ctx, source, &stages, fmap, n, span));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_take(ctx, source, &stages, n, span))
}

fn fuse_hof_take_min(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    m: Expr,
    n: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let a = format!("__take_a_{uid}");
    let b = format!("__take_b_{uid}");
    let lim = format!("__take_n_{uid}");
    let min_e = Expr::If {
        cond: Box::new(Expr::Binary {
            op: BinOp::Lt,
            left: Box::new(Expr::Var(a.clone(), span)),
            right: Box::new(Expr::Var(b.clone(), span)),
            span,
        }),
        then_branch: Box::new(Expr::Var(a.clone(), span)),
        else_branch: Box::new(Expr::Var(b.clone(), span)),
        span,
    };
    // Re-enter so further nested cuts on `inner` still peel.
    let body = match fuse_hof_take_on(ctx, inner, Expr::Var(lim.clone(), span), span) {
        Some(e) => e,
        None => Expr::BuiltinCall {
            name: Builtin::ListTake,
            args: vec![lower_expr(ctx, inner), Expr::Var(lim.clone(), span)],
            span,
        },
    };
    nest_lets(
        vec![(a, m, false), (b, n, false), (lim, min_e, false)],
        body,
    )
}

fn fuse_hof_take_after_drop(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    drop_n: Expr,
    take_n: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let drop_raw = format!("__drop_raw_{uid}");
    let drop_lim = format!("__drop_n_{uid}");
    let take_raw = format!("__take_raw_{uid}");
    let take_lim = format!("__take_n_{uid}");
    let body = fuse_hof_take_after_drop_scan(ctx, inner, &drop_lim, &take_lim, span);
    bind_nonneg_lim(
        drop_raw,
        drop_lim,
        drop_n,
        bind_nonneg_lim(take_raw, take_lim, take_n, body, span),
        span,
    )
}

fn fuse_hof_take_after_drop_scan(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    drop_lim: &str,
    take_lim: &str,
    span: Span,
) -> Expr {
    if let Some((base, fmap)) = peel_trailing_flat_map(inner) {
        let (source, stages) = peel_hof_stages(base);
        return fuse_hof_take_after_drop_flat_map(
            ctx, source, &stages, fmap, drop_lim, take_lim, span,
        );
    }
    let (source, stages) = peel_hof_stages(inner);
    if stages.is_empty() {
        // Fallback: drop then take builtins on lowered inner.
        return Expr::BuiltinCall {
            name: Builtin::ListTake,
            args: vec![
                Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args: vec![
                        lower_expr(ctx, inner),
                        Expr::Var(drop_lim.to_string(), span),
                    ],
                    span,
                },
                Expr::Var(take_lim.to_string(), span),
            ],
            span,
        };
    }
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::MAP_ACC_PREFIX, uid);
    let skipped = format!("__drop_k_{uid}");
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, &stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let append = take_append_step(&acc, &k, take_lim, &x_out, span);
    let body = drop_then(&skipped, drop_lim, append, span);
    let step = take_limit_step(
        &k,
        take_lim,
        wrap_staged_step(body, lets, guards, span),
        span,
    );
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (k, Expr::Int(0, span), true),
            (acc.clone(), empty_list(span), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_take_after_drop_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    drop_lim: &str,
    take_lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::MAP_ACC_PREFIX, uid);
    let skipped = format!("__drop_k_{uid}");
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let append = take_append_step(&acc, &k, take_lim, &y, span);
    let inner_y = drop_then(&skipped, drop_lim, append, span);
    let body = for_each_elem(&y, chunk, inner_y, span);
    let step = take_limit_step(
        &k,
        take_lim,
        wrap_staged_step(body, lets, guards, span),
        span,
    );
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (k, Expr::Int(0, span), true),
            (acc.clone(), empty_list(span), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_take(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    n: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::MAP_ACC_PREFIX, uid);
    let k = format!("__take_k_{uid}");
    let lim = format!("__take_n_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let inner = wrap_staged_step(
        take_append_step(&acc, &k, &lim, &x_out, span),
        lets,
        guards,
        span,
    );
    let step = take_limit_step(&k, &lim, inner, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (lim, n, false),
            (k, Expr::Int(0, span), true),
            (acc.clone(), empty_list(span), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn fuse_hof_take_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    n: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::MAP_ACC_PREFIX, uid);
    let k = format!("__take_k_{uid}");
    let lim = format!("__take_n_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let inner_y = take_limit_step(&k, &lim, take_append_step(&acc, &k, &lim, &y, span), span);
    let body = Expr::Seq {
        stmts: vec![
            for_each_elem(&y, chunk, inner_y, span),
            Expr::If {
                cond: Box::new(take_reached(&k, &lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let inner = wrap_staged_step(body, lets, guards, span);
    let step = take_limit_step(&k, &lim, inner, span);
    let source_e = lower_expr(ctx, source);
    nest_lets(
        vec![
            (lim, n, false),
            (k, Expr::Int(0, span), true),
            (acc.clone(), empty_list(span), true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, source_e, step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

/// Fuse `base.(map|filter)+.drop(n)` / `.slice(n)` — skip `n` survivors then append.
///
/// Peels trailing `.take` / nested `.drop` on `base`:
/// - `take(t).drop(d)` ≡ `drop(d).take(max(0, t-d))` on the same pipe
/// - `drop(a).drop(b)` ≡ `drop(a+b)`
pub(crate) fn try_fuse_hof_drop(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    n: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let n = lower_expr(ctx, n);
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let m = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            TrailingCut::Take(_) => Some(fuse_hof_drop_after_take(ctx, inner, m, n, span)),
            TrailingCut::Drop(_) => Some(fuse_hof_drop_sum(ctx, inner, m, n, span)),
        };
    }
    fuse_hof_drop_on(ctx, base, n, span)
}

fn fuse_hof_drop_on(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    n: Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, fmap)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_drop_flat_map(ctx, source, &stages, fmap, n, span));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_drop(ctx, source, &stages, n, span))
}

fn fuse_hof_drop_sum(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    a: Expr,
    b: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let xa = format!("__drop_a_{uid}");
    let xb = format!("__drop_b_{uid}");
    let lim = format!("__drop_n_{uid}");
    let sum = Expr::Binary {
        op: BinOp::Add,
        left: Box::new(Expr::Var(xa.clone(), span)),
        right: Box::new(Expr::Var(xb.clone(), span)),
        span,
    };
    let body = match fuse_hof_drop_on(ctx, inner, Expr::Var(lim.clone(), span), span) {
        Some(e) => e,
        None => Expr::BuiltinCall {
            name: Builtin::ListSlice,
            args: vec![lower_expr(ctx, inner), Expr::Var(lim.clone(), span)],
            span,
        },
    };
    nest_lets(
        vec![(xa, a, false), (xb, b, false), (lim, sum, false)],
        body,
    )
}

fn fuse_hof_drop_after_take(
    ctx: &LowerCtx,
    inner: &lumia_syntax::Expr,
    take_n: Expr,
    drop_n: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let t_raw = format!("__take_raw_{uid}");
    let t_lim = format!("__take_n_{uid}");
    let d_raw = format!("__drop_raw_{uid}");
    let d_lim = format!("__drop_n_{uid}");
    // remain = max(0, take_n - drop_n); then drop(drop_n).take(remain).
    let remain = format!("__take_remain_{uid}");
    let remain_e = Expr::If {
        cond: Box::new(Expr::Binary {
            op: BinOp::Ge,
            left: Box::new(Expr::Var(d_lim.clone(), span)),
            right: Box::new(Expr::Var(t_lim.clone(), span)),
            span,
        }),
        then_branch: Box::new(Expr::Int(0, span)),
        else_branch: Box::new(Expr::Binary {
            op: BinOp::Sub,
            left: Box::new(Expr::Var(t_lim.clone(), span)),
            right: Box::new(Expr::Var(d_lim.clone(), span)),
            span,
        }),
        span,
    };
    let body = fuse_hof_take_after_drop_scan(ctx, inner, &d_lim, &remain, span);
    bind_nonneg_lim(
        t_raw,
        t_lim,
        take_n,
        bind_nonneg_lim(
            d_raw,
            d_lim,
            drop_n,
            Expr::Let {
                name: remain,
                value: Box::new(remain_e),
                body: Box::new(body),
                mutable: false,
                ty: None,
            },
            span,
        ),
        span,
    )
}

fn fuse_hof_drop(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    n: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::MAP_ACC_PREFIX, uid);
    let raw = format!("__drop_raw_{uid}");
    let lim = format!("__drop_n_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let body = drop_then(
        &skipped,
        &lim,
        append_assign(&acc, Expr::Var(x_out, span), span),
        span,
    );
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    bind_nonneg_lim(
        raw,
        lim,
        n,
        nest_lets(
            vec![
                (skipped, Expr::Int(0, span), true),
                (acc.clone(), empty_list(span), true),
            ],
            Expr::Seq {
                stmts: vec![
                    for_each_elem(&x0, source_e, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            },
        ),
        span,
    )
}

fn fuse_hof_drop_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    n: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let acc = format!("{}_{}", crate::desugar_slots::MAP_ACC_PREFIX, uid);
    let raw = format!("__drop_raw_{uid}");
    let lim = format!("__drop_n_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let inner_y = drop_then(
        &skipped,
        &lim,
        append_assign(&acc, Expr::Var(y.clone(), span), span),
        span,
    );
    let body = for_each_elem(&y, chunk, inner_y, span);
    let step = wrap_staged_step(body, lets, guards, span);
    let source_e = lower_expr(ctx, source);
    bind_nonneg_lim(
        raw,
        lim,
        n,
        nest_lets(
            vec![
                (skipped, Expr::Int(0, span), true),
                (acc.clone(), empty_list(span), true),
            ],
            Expr::Seq {
                stmts: vec![
                    for_each_elem(&x0, source_e, step, span),
                    Expr::Var(acc, span),
                ],
                span,
            },
        ),
        span,
    )
}

fn take_reached(k: &str, lim: &str, span: Span) -> Expr {
    Expr::Binary {
        op: BinOp::Ge,
        left: Box::new(Expr::Var(k.to_string(), span)),
        right: Box::new(Expr::Var(lim.to_string(), span)),
        span,
    }
}

fn take_limit_step(k: &str, lim: &str, then_branch: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(take_reached(k, lim, span)),
        then_branch: Box::new(Expr::Break(span)),
        else_branch: Box::new(then_branch),
        span,
    }
}

fn take_append_step(acc: &str, k: &str, lim: &str, elem: &str, span: Span) -> Expr {
    Expr::Seq {
        stmts: vec![
            append_assign(acc, Expr::Var(elem.to_string(), span), span),
            Expr::Assign {
                name: k.to_string(),
                value: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    left: Box::new(Expr::Var(k.to_string(), span)),
                    right: Box::new(Expr::Int(1, span)),
                    span,
                }),
                span,
            },
            Expr::If {
                cond: Box::new(take_reached(k, lim, span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    }
}

fn get_scan_step(ctx: &LowerCtx, seen: &str, acc: &str, idx: &str, elem: &str, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(Expr::Var(seen.to_string(), span)),
            right: Box::new(Expr::Var(idx.to_string(), span)),
            span,
        }),
        then_branch: Box::new(Expr::Seq {
            stmts: vec![
                Expr::Assign {
                    name: acc.to_string(),
                    value: Box::new(crate::list_hof::option_some(
                        ctx,
                        Expr::Var(elem.to_string(), span),
                        span,
                    )),
                    span,
                },
                Expr::Break(span),
            ],
            span,
        }),
        else_branch: Box::new(Expr::Assign {
            name: seen.to_string(),
            value: Box::new(Expr::Binary {
                op: BinOp::Add,
                left: Box::new(Expr::Var(seen.to_string(), span)),
                right: Box::new(Expr::Int(1, span)),
                span,
            }),
            span,
        }),
        span,
    }
}

fn nest_lets(binds: Vec<(String, Expr, bool)>, body: Expr) -> Expr {
    binds
        .into_iter()
        .rev()
        .fold(body, |body, (name, value, mutable)| Expr::Let {
            name,
            value: Box::new(value),
            body: Box::new(body),
            mutable,
            ty: None,
        })
}

fn is_some_var(ctx: &LowerCtx, acc: &str, span: Span) -> Expr {
    let some_tag = ctx.lookup_ctor("Some").map(|c| c.tag).unwrap_or(0);
    Expr::Binary {
        op: BinOp::Eq,
        left: Box::new(Expr::BuiltinCall {
            name: Builtin::AdtTag,
            args: vec![Expr::Var(acc.to_string(), span)],
            span,
        }),
        right: Box::new(Expr::Int(some_tag, span)),
        span,
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
            args: vec![acc, Expr::Int(0, span), Expr::String("Some".into(), span)],
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

/// `for x in pipe.(map|filter)+ { body }` — sequential scan, no list (and no
/// `ListParMap`). Trailing `take`/`drop` increment the cut counter **before**
/// the user body so `continue` still counts. Trailing `flatMap` rewrites
/// for-in `break` (not nested `Loop` breaks) into a done flag so it exits both
/// loops.
pub(crate) fn try_fuse_hof_for_in(
    ctx: &LowerCtx,
    binding: &str,
    iter: &lumia_syntax::Expr,
    body: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    let body_e = lower_expr(ctx, body);
    if let Some((inner, cut)) = peel_trailing_take_drop(iter) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        return match cut {
            TrailingCut::Take(_) => {
                fuse_hof_for_in_under_take(ctx, binding, inner, n, body_e, span)
            }
            TrailingCut::Drop(_) => {
                fuse_hof_for_in_under_drop(ctx, binding, inner, n, body_e, span)
            }
        };
    }
    if let Some((inner, fmap)) = peel_trailing_flat_map(iter) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_for_in_flat_map(
            ctx, binding, source, &stages, fmap, body_e, span,
        ));
    }
    let (source, stages) = peel_hof_stages(iter);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_for_in(ctx, binding, source, &stages, body_e, span))
}

fn fuse_hof_for_in(
    ctx: &LowerCtx,
    binding: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    user_body: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let x0 = format!("__fuse_x_{uid}");
    let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    let body = Expr::Let {
        name: binding.to_string(),
        value: Box::new(cur),
        body: Box::new(user_body),
        mutable: false,
        ty: None,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    for_each_elem(&x0, lower_expr(ctx, source), step, span)
}

fn fuse_hof_for_in_under_take(
    ctx: &LowerCtx,
    binding: &str,
    inner: &lumia_syntax::Expr,
    n: Expr,
    user_body: Expr,
    span: Span,
) -> Option<Expr> {
    if let Expr::Int(k, _) = n {
        if k <= 0 {
            return Some(Expr::Unit(span));
        }
    }
    let uid = span.start.0;
    let raw = format!("__take_raw_{uid}");
    let lim = format!("__take_n_{uid}");
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(f) => {
            fuse_hof_for_in_flat_map_capped(ctx, binding, source, &stages, f, user_body, &lim, span)
        }
        None => fuse_hof_for_in_capped(ctx, binding, source, &stages, user_body, &lim, span),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
}

fn fuse_hof_for_in_under_drop(
    ctx: &LowerCtx,
    binding: &str,
    inner: &lumia_syntax::Expr,
    n: Expr,
    user_body: Expr,
    span: Span,
) -> Option<Expr> {
    if matches!(n, Expr::Int(0, _)) {
        if let Some((base, fmap)) = peel_trailing_flat_map(inner) {
            let (source, stages) = peel_hof_stages(base);
            return Some(fuse_hof_for_in_flat_map(
                ctx, binding, source, &stages, fmap, user_body, span,
            ));
        }
        let (source, stages) = peel_hof_stages(inner);
        if stages.is_empty() {
            return None;
        }
        return Some(fuse_hof_for_in(
            ctx, binding, source, &stages, user_body, span,
        ));
    }
    let uid = span.start.0;
    let raw = format!("__drop_raw_{uid}");
    let lim = format!("__drop_n_{uid}");
    let (source, stages, fmap) = peel_len_base(inner)?;
    let body = match fmap {
        Some(f) => {
            fuse_hof_for_in_flat_map_skip(ctx, binding, source, &stages, f, user_body, &lim, span)
        }
        None => fuse_hof_for_in_skip(ctx, binding, source, &stages, user_body, &lim, span),
    };
    Some(bind_nonneg_lim(raw, lim, n, body, span))
}

fn bind_user(binding: &str, elem: Expr, user_body: Expr, span: Span) -> Expr {
    Expr::Let {
        name: binding.to_string(),
        value: Box::new(elem),
        body: Box::new(user_body),
        mutable: false,
        ty: None,
    }
}

fn k_inc_then(k: &str, then_branch: Expr, span: Span) -> Expr {
    Expr::Seq {
        stmts: vec![
            Expr::Assign {
                name: k.to_string(),
                value: Box::new(Expr::Binary {
                    op: BinOp::Add,
                    left: Box::new(Expr::Var(k.to_string(), span)),
                    right: Box::new(Expr::Int(1, span)),
                    span,
                }),
                span,
            },
            then_branch,
        ],
        span,
    }
}

fn fuse_hof_for_in_capped(
    ctx: &LowerCtx,
    binding: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    user_body: Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    let inner = k_inc_then(&k, bind_user(binding, cur, user_body, span), span);
    let step = take_limit_step(&k, lim, wrap_staged_step(inner, lets, guards, span), span);
    nest_lets(
        vec![(k, Expr::Int(0, span), true)],
        for_each_elem(&x0, lower_expr(ctx, source), step, span),
    )
}

fn fuse_hof_for_in_skip(
    ctx: &LowerCtx,
    binding: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    user_body: Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    let inner = drop_then(
        &skipped,
        lim,
        bind_user(binding, cur, user_body, span),
        span,
    );
    let step = wrap_staged_step(inner, lets, guards, span);
    nest_lets(
        vec![(skipped, Expr::Int(0, span), true)],
        for_each_elem(&x0, lower_expr(ctx, source), step, span),
    )
}

fn fuse_hof_for_in_flat_map(
    ctx: &LowerCtx,
    binding: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    user_body: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let done = format!("__for_done_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let user = rewrite_for_in_breaks(&user_body, &done, span);
    let inner_y = bind_user(binding, Expr::Var(y.clone(), span), user, span);
    let inner = for_each_elem(&y, chunk, inner_y, span);
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(Expr::Var(done.clone(), span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    Expr::Let {
        name: done.clone(),
        value: Box::new(Expr::Bool(false, span)),
        body: Box::new(for_each_elem(&x0, lower_expr(ctx, source), step, span)),
        mutable: true,
        ty: None,
    }
}

fn fuse_hof_for_in_flat_map_capped(
    ctx: &LowerCtx,
    binding: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    user_body: Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let done = format!("__for_done_{uid}");
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let user = rewrite_for_in_breaks(&user_body, &done, span);
    let inner_y = take_limit_step(
        &k,
        lim,
        k_inc_then(
            &k,
            bind_user(binding, Expr::Var(y.clone(), span), user, span),
            span,
        ),
        span,
    );
    let inner = for_each_elem(&y, chunk, inner_y, span);
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(Expr::Binary {
                    op: BinOp::Or,
                    left: Box::new(Expr::Var(done.clone(), span)),
                    right: Box::new(take_reached(&k, lim, span)),
                    span,
                }),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let step = take_limit_step(&k, lim, wrap_staged_step(body, lets, guards, span), span);
    nest_lets(
        vec![
            (done, Expr::Bool(false, span), true),
            (k, Expr::Int(0, span), true),
        ],
        for_each_elem(&x0, lower_expr(ctx, source), step, span),
    )
}

fn fuse_hof_for_in_flat_map_skip(
    ctx: &LowerCtx,
    binding: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    user_body: Expr,
    lim: &str,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let done = format!("__for_done_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let user = rewrite_for_in_breaks(&user_body, &done, span);
    let inner_y = drop_then(
        &skipped,
        lim,
        bind_user(binding, Expr::Var(y.clone(), span), user, span),
        span,
    );
    let inner = for_each_elem(&y, chunk, inner_y, span);
    let body = Expr::Seq {
        stmts: vec![
            inner,
            Expr::If {
                cond: Box::new(Expr::Var(done.clone(), span)),
                then_branch: Box::new(Expr::Break(span)),
                else_branch: Box::new(Expr::Unit(span)),
                span,
            },
        ],
        span,
    };
    let step = wrap_staged_step(body, lets, guards, span);
    nest_lets(
        vec![
            (done, Expr::Bool(false, span), true),
            (skipped, Expr::Int(0, span), true),
        ],
        for_each_elem(&x0, lower_expr(ctx, source), step, span),
    )
}

/// Rewrite for-in `break` to set `done` (so a fused `flatMap` can exit the outer
/// scan). Nested `Loop` breaks are left alone.
fn rewrite_for_in_breaks(e: &Expr, done: &str, span: Span) -> Expr {
    match e {
        Expr::Break(s) => Expr::Seq {
            stmts: vec![
                Expr::Assign {
                    name: done.to_string(),
                    value: Box::new(Expr::Bool(true, span)),
                    span: *s,
                },
                Expr::Break(*s),
            ],
            span: *s,
        },
        Expr::Loop { .. } => e.clone(),
        Expr::Let {
            name,
            value,
            body,
            mutable,
            ty,
        } => Expr::Let {
            name: name.clone(),
            value: Box::new(rewrite_for_in_breaks(value, done, span)),
            body: Box::new(rewrite_for_in_breaks(body, done, span)),
            mutable: *mutable,
            ty: ty.clone(),
        },
        Expr::Assign {
            name,
            value,
            span: s,
        } => Expr::Assign {
            name: name.clone(),
            value: Box::new(rewrite_for_in_breaks(value, done, span)),
            span: *s,
        },
        Expr::Seq { stmts, span: s } => Expr::Seq {
            stmts: stmts
                .iter()
                .map(|st| rewrite_for_in_breaks(st, done, span))
                .collect(),
            span: *s,
        },
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span: s,
        } => Expr::If {
            cond: Box::new(rewrite_for_in_breaks(cond, done, span)),
            then_branch: Box::new(rewrite_for_in_breaks(then_branch, done, span)),
            else_branch: Box::new(rewrite_for_in_breaks(else_branch, done, span)),
            span: *s,
        },
        Expr::Call {
            callee,
            args,
            span: s,
        } => Expr::Call {
            callee: Box::new(rewrite_for_in_breaks(callee, done, span)),
            args: args
                .iter()
                .map(|a| rewrite_for_in_breaks(a, done, span))
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
            left: Box::new(rewrite_for_in_breaks(left, done, span)),
            right: Box::new(rewrite_for_in_breaks(right, done, span)),
            span: *s,
        },
        Expr::Unary { op, expr, span: s } => Expr::Unary {
            op: *op,
            expr: Box::new(rewrite_for_in_breaks(expr, done, span)),
            span: *s,
        },
        Expr::Return { value, span: s } => Expr::Return {
            value: Box::new(rewrite_for_in_breaks(value, done, span)),
            span: *s,
        },
        Expr::BuiltinCall {
            name,
            args,
            span: s,
        } => Expr::BuiltinCall {
            name: *name,
            args: args
                .iter()
                .map(|a| rewrite_for_in_breaks(a, done, span))
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
                .map(|a| rewrite_for_in_breaks(a, done, span))
                .collect(),
            span: *s,
        },
        Expr::Lambda {
            params,
            param_ann,
            body,
            span: s,
        } => Expr::Lambda {
            params: params.clone(),
            param_ann: param_ann.clone(),
            body: Box::new(rewrite_for_in_breaks(body, done, span)),
            span: *s,
        },
        other => other.clone(),
    }
}

/// `base.(map|filter)+.toList()` — one builder (skip a second copy pass).
pub(crate) fn try_fuse_hof_to_list(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        return match cut {
            TrailingCut::Take(n) => try_fuse_hof_take(ctx, inner, n, span),
            TrailingCut::Drop(n) => try_fuse_hof_drop(ctx, inner, n, span),
        };
    }
    if let Some((inner, fmap)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(fuse_hof_flat_map(ctx, source, &stages, fmap, span));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(fuse_hof_build(ctx, source, &stages, span))
}

/// `base.(map|filter)+.toSet()` — insert survivors, no intermediate List.
pub(crate) fn try_fuse_hof_to_set(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    fuse_hof_collect(ctx, base, span, CollectKind::Set)
}

#[derive(Clone, Copy)]
enum TomapLookup {
    /// Last-wins `Some(v)` / `None` — full scan (DESIGN §7.3.1).
    Get,
    /// Key membership; first hit may break.
    Contains,
}

/// `pipe.toMap().get(k)` / `.contains(k)` — scan the pair stream, no Hash map.
fn fuse_tomap_lookup(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    key: Expr,
    span: Span,
    kind: TomapLookup,
) -> Option<Expr> {
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        let uid = span.start.0;
        return match cut {
            TrailingCut::Take(_) => {
                let raw = format!("__take_raw_{uid}");
                let lim = format!("__take_n_{uid}");
                let (source, stages, fmap) = peel_stream_base(inner);
                let body = tomap_lookup_capped(ctx, source, &stages, fmap, &lim, key, span, kind);
                Some(bind_nonneg_lim(raw, lim, n, body, span))
            }
            TrailingCut::Drop(_) => {
                let raw = format!("__drop_raw_{uid}");
                let lim = format!("__drop_n_{uid}");
                let (source, stages, fmap) = peel_stream_base(inner);
                let body = tomap_lookup_skip(ctx, source, &stages, fmap, &lim, key, span, kind);
                Some(bind_nonneg_lim(raw, lim, n, body, span))
            }
        };
    }
    if let Some((inner, fmap)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(tomap_lookup_flat_map(
            ctx, source, &stages, fmap, key, span, kind,
        ));
    }
    let (source, stages) = peel_hof_stages(base);
    Some(tomap_lookup_plain(ctx, source, &stages, key, span, kind))
}

/// Like [`peel_len_base`], but a bare source (no map/filter) is still a stream.
fn peel_stream_base<'a>(
    base: &'a lumia_syntax::Expr,
) -> (
    &'a lumia_syntax::Expr,
    Vec<HofStage<'a>>,
    Option<&'a lumia_syntax::Expr>,
) {
    if let Some((inner, f)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return (source, stages, Some(f));
    }
    let (source, stages) = peel_hof_stages(base);
    (source, stages, None)
}

fn tomap_lookup_acc(ctx: &LowerCtx, kind: TomapLookup, uid: u32, span: Span) -> (String, Expr) {
    match kind {
        TomapLookup::Get => (
            format!("__mget_acc_{uid}"),
            crate::list_hof::option_none(ctx, span),
        ),
        TomapLookup::Contains => (format!("__mcontains_acc_{uid}"), Expr::Bool(false, span)),
    }
}

fn pair_field(p: &str, idx: i64, span: Span) -> Expr {
    Expr::BuiltinCall {
        name: Builtin::AdtField,
        args: vec![Expr::Var(p.to_string(), span), Expr::Int(idx, span)],
        span,
    }
}

fn tomap_lookup_update(
    ctx: &LowerCtx,
    kind: TomapLookup,
    acc: &str,
    elem: Expr,
    key: &str,
    span: Span,
) -> Expr {
    let p = format!("__mget_p_{}", span.start.0);
    let key_eq = Expr::Binary {
        op: BinOp::Eq,
        left: Box::new(pair_field(&p, 0, span)),
        right: Box::new(Expr::Var(key.to_string(), span)),
        span,
    };
    let hit = match kind {
        TomapLookup::Get => Expr::Assign {
            name: acc.to_string(),
            value: Box::new(crate::list_hof::option_some(
                ctx,
                pair_field(&p, 1, span),
                span,
            )),
            span,
        },
        TomapLookup::Contains => Expr::Seq {
            stmts: vec![
                Expr::Assign {
                    name: acc.to_string(),
                    value: Box::new(Expr::Bool(true, span)),
                    span,
                },
                Expr::Break(span),
            ],
            span,
        },
    };
    Expr::Let {
        name: p,
        value: Box::new(elem),
        body: Box::new(Expr::If {
            cond: Box::new(key_eq),
            then_branch: Box::new(hit),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        }),
        mutable: false,
        ty: None,
    }
}

fn wrap_key_scan(kn: String, key: Expr, inner: Expr, _span: Span) -> Expr {
    Expr::Let {
        name: kn,
        value: Box::new(key),
        body: Box::new(inner),
        mutable: false,
        ty: None,
    }
}

fn tomap_lookup_plain(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    key: Expr,
    span: Span,
    kind: TomapLookup,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = tomap_lookup_acc(ctx, kind, uid, span);
    let kn = format!("__mget_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    let step = wrap_staged_step(
        tomap_lookup_update(ctx, kind, &acc, cur, &kn, span),
        lets,
        guards,
        span,
    );
    wrap_key_scan(
        kn,
        key,
        Expr::Let {
            name: acc.clone(),
            value: Box::new(init),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    for_each_elem(&x0, lower_expr(ctx, source), step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
            ty: None,
        },
        span,
    )
}

fn tomap_lookup_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    lim: &str,
    key: Expr,
    span: Span,
    kind: TomapLookup,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = tomap_lookup_acc(ctx, kind, uid, span);
    let kn = format!("__mget_k_{uid}");
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let step = match fmap {
        Some(f) => {
            let x_out = format!("__fuse_xm_{uid}");
            let y = format!("__fuse_y_{uid}");
            let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            lets.push((x_out.clone(), cur));
            let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
            let inner_y = take_limit_step(
                &k,
                lim,
                k_inc_then(
                    &k,
                    tomap_lookup_update(ctx, kind, &acc, Expr::Var(y.clone(), span), &kn, span),
                    span,
                ),
                span,
            );
            let inner = for_each_elem(&y, chunk, inner_y, span);
            let body = Expr::Seq {
                stmts: vec![
                    inner,
                    Expr::If {
                        cond: Box::new(take_reached(&k, lim, span)),
                        then_branch: Box::new(Expr::Break(span)),
                        else_branch: Box::new(Expr::Unit(span)),
                        span,
                    },
                ],
                span,
            };
            take_limit_step(&k, lim, wrap_staged_step(body, lets, guards, span), span)
        }
        None => {
            let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            let inner = k_inc_then(
                &k,
                tomap_lookup_update(ctx, kind, &acc, cur, &kn, span),
                span,
            );
            take_limit_step(&k, lim, wrap_staged_step(inner, lets, guards, span), span)
        }
    };
    wrap_key_scan(
        kn,
        key,
        nest_lets(
            vec![(k, Expr::Int(0, span), true), (acc.clone(), init, true)],
            Expr::Seq {
                stmts: vec![
                    for_each_elem(&x0, lower_expr(ctx, source), step, span),
                    Expr::Var(acc, span),
                ],
                span,
            },
        ),
        span,
    )
}

fn tomap_lookup_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    lim: &str,
    key: Expr,
    span: Span,
    kind: TomapLookup,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = tomap_lookup_acc(ctx, kind, uid, span);
    let kn = format!("__mget_k_{uid}");
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let step = match fmap {
        Some(f) => {
            let x_out = format!("__fuse_xm_{uid}");
            let y = format!("__fuse_y_{uid}");
            let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            lets.push((x_out.clone(), cur));
            let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
            let inner_y = drop_then(
                &skipped,
                lim,
                tomap_lookup_update(ctx, kind, &acc, Expr::Var(y.clone(), span), &kn, span),
                span,
            );
            wrap_staged_step(for_each_elem(&y, chunk, inner_y, span), lets, guards, span)
        }
        None => {
            let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            wrap_staged_step(
                drop_then(
                    &skipped,
                    lim,
                    tomap_lookup_update(ctx, kind, &acc, cur, &kn, span),
                    span,
                ),
                lets,
                guards,
                span,
            )
        }
    };
    wrap_key_scan(
        kn,
        key,
        nest_lets(
            vec![
                (skipped, Expr::Int(0, span), true),
                (acc.clone(), init, true),
            ],
            Expr::Seq {
                stmts: vec![
                    for_each_elem(&x0, lower_expr(ctx, source), step, span),
                    Expr::Var(acc, span),
                ],
                span,
            },
        ),
        span,
    )
}

fn tomap_lookup_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    key: Expr,
    span: Span,
    kind: TomapLookup,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = tomap_lookup_acc(ctx, kind, uid, span);
    let kn = format!("__mget_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let inner = for_each_elem(
        &y,
        chunk,
        tomap_lookup_update(ctx, kind, &acc, Expr::Var(y.clone(), span), &kn, span),
        span,
    );
    let step = wrap_staged_step(inner, lets, guards, span);
    wrap_key_scan(
        kn,
        key,
        Expr::Let {
            name: acc.clone(),
            value: Box::new(init),
            body: Box::new(Expr::Seq {
                stmts: vec![
                    for_each_elem(&x0, lower_expr(ctx, source), step, span),
                    Expr::Var(acc, span),
                ],
                span,
            }),
            mutable: true,
            ty: None,
        },
        span,
    )
}

/// `base.(map|filter)+.toMap()` — `MapSet` on pair survivors, no intermediate List.
pub(crate) fn try_fuse_hof_to_map(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    span: Span,
) -> Option<Expr> {
    fuse_hof_collect(ctx, base, span, CollectKind::Map)
}

#[derive(Clone, Copy)]
enum CollectKind {
    Set,
    Map,
}

fn fuse_hof_collect(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    span: Span,
    kind: CollectKind,
) -> Option<Expr> {
    if let Some((inner, cut)) = peel_trailing_take_drop(base) {
        let n = lower_expr(
            ctx,
            match &cut {
                TrailingCut::Take(e) | TrailingCut::Drop(e) => e,
            },
        );
        let uid = span.start.0;
        return match cut {
            TrailingCut::Take(_) => {
                let raw = format!("__take_raw_{uid}");
                let lim = format!("__take_n_{uid}");
                let (source, stages, fmap) = peel_len_base(inner)?;
                let body = collect_capped(ctx, source, &stages, fmap, &lim, span, kind);
                Some(bind_nonneg_lim(raw, lim, n, body, span))
            }
            TrailingCut::Drop(_) => {
                let raw = format!("__drop_raw_{uid}");
                let lim = format!("__drop_n_{uid}");
                let (source, stages, fmap) = peel_len_base(inner)?;
                let body = collect_skip(ctx, source, &stages, fmap, &lim, span, kind);
                Some(bind_nonneg_lim(raw, lim, n, body, span))
            }
        };
    }
    if let Some((inner, fmap)) = peel_trailing_flat_map(base) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(collect_flat_map(ctx, source, &stages, fmap, span, kind));
    }
    let (source, stages) = peel_hof_stages(base);
    if stages.is_empty() {
        return None;
    }
    Some(collect_plain(ctx, source, &stages, span, kind))
}

fn collect_acc_init(kind: CollectKind, uid: u32, span: Span) -> (String, Expr) {
    match kind {
        CollectKind::Set => (
            format!("{}_{}", crate::desugar_slots::TOSET_ACC_PREFIX, uid),
            empty_set(span),
        ),
        CollectKind::Map => (
            format!("{}_{}", crate::desugar_slots::TOMAP_ACC_PREFIX, uid),
            empty_map(span),
        ),
    }
}

fn collect_insert(kind: CollectKind, acc: &str, elem: Expr, span: Span) -> Expr {
    match kind {
        CollectKind::Set => Expr::Assign {
            name: acc.to_string(),
            value: Box::new(Expr::BuiltinCall {
                name: Builtin::SetInsert,
                args: vec![Expr::Var(acc.to_string(), span), elem],
                span,
            }),
            span,
        },
        CollectKind::Map => {
            let p = format!("__tomap_p_{}", span.start.0);
            Expr::Let {
                name: p.clone(),
                value: Box::new(elem),
                body: Box::new(Expr::Assign {
                    name: acc.to_string(),
                    value: Box::new(Expr::BuiltinCall {
                        name: Builtin::MapSet,
                        args: vec![
                            Expr::Var(acc.to_string(), span),
                            Expr::BuiltinCall {
                                name: Builtin::AdtField,
                                args: vec![Expr::Var(p.clone(), span), Expr::Int(0, span)],
                                span,
                            },
                            Expr::BuiltinCall {
                                name: Builtin::AdtField,
                                args: vec![Expr::Var(p, span), Expr::Int(1, span)],
                                span,
                            },
                        ],
                        span,
                    }),
                    span,
                }),
                mutable: false,
                ty: None,
            }
        }
    }
}

fn collect_plain(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    span: Span,
    kind: CollectKind,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = collect_acc_init(kind, uid, span);
    let x0 = format!("__fuse_x_{uid}");
    let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    let step = wrap_staged_step(collect_insert(kind, &acc, cur, span), lets, guards, span);
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, lower_expr(ctx, source), step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

fn collect_capped(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    lim: &str,
    span: Span,
    kind: CollectKind,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = collect_acc_init(kind, uid, span);
    let k = format!("__take_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let step = match fmap {
        Some(f) => {
            let x_out = format!("__fuse_xm_{uid}");
            let y = format!("__fuse_y_{uid}");
            let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            lets.push((x_out.clone(), cur));
            let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
            let inner_y = take_limit_step(
                &k,
                lim,
                k_inc_then(
                    &k,
                    collect_insert(kind, &acc, Expr::Var(y.clone(), span), span),
                    span,
                ),
                span,
            );
            let inner = for_each_elem(&y, chunk, inner_y, span);
            let body = Expr::Seq {
                stmts: vec![
                    inner,
                    Expr::If {
                        cond: Box::new(take_reached(&k, lim, span)),
                        then_branch: Box::new(Expr::Break(span)),
                        else_branch: Box::new(Expr::Unit(span)),
                        span,
                    },
                ],
                span,
            };
            take_limit_step(&k, lim, wrap_staged_step(body, lets, guards, span), span)
        }
        None => {
            let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            let inner = k_inc_then(&k, collect_insert(kind, &acc, cur, span), span);
            take_limit_step(&k, lim, wrap_staged_step(inner, lets, guards, span), span)
        }
    };
    nest_lets(
        vec![(k, Expr::Int(0, span), true), (acc.clone(), init, true)],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, lower_expr(ctx, source), step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn collect_skip(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    lim: &str,
    span: Span,
    kind: CollectKind,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = collect_acc_init(kind, uid, span);
    let skipped = format!("__drop_k_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let step = match fmap {
        Some(f) => {
            let x_out = format!("__fuse_xm_{uid}");
            let y = format!("__fuse_y_{uid}");
            let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            lets.push((x_out.clone(), cur));
            let chunk = apply_hof_fn(ctx, f, Expr::Var(x_out, span), span);
            let inner_y = drop_then(
                &skipped,
                lim,
                collect_insert(kind, &acc, Expr::Var(y.clone(), span), span),
                span,
            );
            wrap_staged_step(for_each_elem(&y, chunk, inner_y, span), lets, guards, span)
        }
        None => {
            let (cur, lets, guards) = stage_pipeline(ctx, stages, &x0, span);
            wrap_staged_step(
                drop_then(&skipped, lim, collect_insert(kind, &acc, cur, span), span),
                lets,
                guards,
                span,
            )
        }
    };
    nest_lets(
        vec![
            (skipped, Expr::Int(0, span), true),
            (acc.clone(), init, true),
        ],
        Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, lower_expr(ctx, source), step, span),
                Expr::Var(acc, span),
            ],
            span,
        },
    )
}

fn collect_flat_map(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: &lumia_syntax::Expr,
    span: Span,
    kind: CollectKind,
) -> Expr {
    let uid = span.start.0;
    let (acc, init) = collect_acc_init(kind, uid, span);
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let y = format!("__fuse_y_{uid}");
    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));
    let chunk = apply_hof_fn(ctx, fmap, Expr::Var(x_out, span), span);
    let inner = for_each_elem(
        &y,
        chunk,
        collect_insert(kind, &acc, Expr::Var(y.clone(), span), span),
        span,
    );
    let step = wrap_staged_step(inner, lets, guards, span);
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(&x0, lower_expr(ctx, source), step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
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
    if pipe_used_under_loop(body, name) {
        return None;
    }
    // DESIGN §7.3.1: unescaped single Map get/contains → scan pairs, no Hash.
    if let Some(inner) = peel_trailing_to_map(value) {
        return try_deforest_tomap_let(ctx, name, inner, body, span);
    }
    if let Some(inner) = peel_trailing_to_set(value) {
        return try_deforest_toset_let(ctx, name, inner, body, span);
    }
    if !pipe_consumers_only(body, name) {
        return None;
    }
    if let Some((inner, fmap)) = peel_trailing_flat_map(value) {
        let (source, stages) = peel_hof_stages(inner);
        return Some(rewrite_pipe_consumers(
            ctx,
            body,
            name,
            source,
            &stages,
            Some(fmap),
            span,
        ));
    }
    let (source, stages) = peel_hof_stages(value);
    if stages.is_empty() {
        return None;
    }
    // Lone `.map` must still materialize (par_map / `$Float` clones). DESIGN §7.3
    // is range / map·filter then indexed, or a filter stage.
    if !let_deforest_pipe(source, &stages) {
        return None;
    }
    let gets = collect_pipe_gets(body, name);
    let has_len = pipe_has_len(body, name);
    // ≥2 gets, or get+len: one scan (len needs the full walk, so no early break).
    if !gets.is_empty() && (gets.len() >= 2 || has_len) {
        return Some(rewrite_shared_gets(
            ctx, body, name, source, &stages, &gets, has_len, span,
        ));
    }
    Some(rewrite_pipe_consumers(
        ctx, body, name, source, &stages, None, span,
    ))
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

/// Indexed get inside a loop cannot share a pre-scan (`ys.get(i)` as `i` mutates).
fn pipe_used_under_loop(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Loop {
            cond, body, step, ..
        } => {
            expr_mentions_var(cond, name)
                || expr_mentions_var(body, name)
                || step.as_ref().is_some_and(|s| expr_mentions_var(s, name))
        }
        Expr::Let {
            name: bind,
            value,
            body,
            ..
        } => {
            pipe_used_under_loop(value, name) || (bind != name && pipe_used_under_loop(body, name))
        }
        Expr::Assign { value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Return { value, .. }
        | Expr::Lambda { body: value, .. } => pipe_used_under_loop(value, name),
        Expr::Seq { stmts, .. } => stmts.iter().any(|s| pipe_used_under_loop(s, name)),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            pipe_used_under_loop(cond, name)
                || pipe_used_under_loop(then_branch, name)
                || pipe_used_under_loop(else_branch, name)
        }
        Expr::Call { callee, args, .. } => {
            pipe_used_under_loop(callee, name) || args.iter().any(|a| pipe_used_under_loop(a, name))
        }
        Expr::Binary { left, right, .. } => {
            pipe_used_under_loop(left, name) || pipe_used_under_loop(right, name)
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            args.iter().any(|a| pipe_used_under_loop(a, name))
        }
        Expr::Alt { scrutinee, alt, .. } => {
            pipe_used_under_loop(scrutinee, name) || pipe_used_under_loop(alt, name)
        }
        Expr::With { base, fields, .. } => {
            pipe_used_under_loop(base, name)
                || fields.iter().any(|(_, v)| pipe_used_under_loop(v, name))
        }
        _ => false,
    }
}

fn pipe_consumers_only(e: &Expr, name: &str) -> bool {
    named_consumers_only(e, name, |b| {
        matches!(
            b,
            Builtin::ListGet
                | Builtin::ListLen
                | Builtin::ListTake
                | Builtin::ListSlice
                | Builtin::Contains
                | Builtin::Elems
        )
    })
}

fn map_lookup_consumers_only(e: &Expr, name: &str) -> bool {
    named_consumers_only(e, name, |b| {
        matches!(b, Builtin::ListGet | Builtin::Contains)
    })
}

fn set_lookup_consumers_only(e: &Expr, name: &str) -> bool {
    named_consumers_only(e, name, |b| matches!(b, Builtin::Contains))
}

fn named_consumers_only(e: &Expr, name: &str, allow: impl Fn(Builtin) -> bool + Copy) -> bool {
    match e {
        Expr::Var(n, _) if n == name => false,
        Expr::BuiltinCall { name: b, args, .. }
            if allow(*b)
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
                named_consumers_only(value, name, allow) && named_consumers_only(body, name, allow)
            }
        }
        Expr::Assign { value, .. } => named_consumers_only(value, name, allow),
        Expr::Seq { stmts, .. } => stmts.iter().all(|s| named_consumers_only(s, name, allow)),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            named_consumers_only(cond, name, allow)
                && named_consumers_only(then_branch, name, allow)
                && named_consumers_only(else_branch, name, allow)
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            named_consumers_only(cond, name, allow)
                && named_consumers_only(body, name, allow)
                && step
                    .as_ref()
                    .map(|s| named_consumers_only(s, name, allow))
                    .unwrap_or(true)
        }
        Expr::Call { callee, args, .. } => {
            named_consumers_only(callee, name, allow)
                && args.iter().all(|a| named_consumers_only(a, name, allow))
        }
        Expr::Binary { left, right, .. } => {
            named_consumers_only(left, name, allow) && named_consumers_only(right, name, allow)
        }
        Expr::Unary { expr, .. } => named_consumers_only(expr, name, allow),
        Expr::Lambda { body, .. } => !expr_mentions_var(body, name),
        Expr::Return { value, .. } => named_consumers_only(value, name, allow),
        Expr::BuiltinCall { args, .. } => args.iter().all(|a| named_consumers_only(a, name, allow)),
        Expr::AdtNew { args, .. } => args.iter().all(|a| named_consumers_only(a, name, allow)),
        _ => !expr_mentions_var(e, name),
    }
}

fn try_deforest_tomap_let(
    ctx: &LowerCtx,
    name: &str,
    inner: &lumia_syntax::Expr,
    body: &Expr,
    span: Span,
) -> Option<Expr> {
    if !map_lookup_consumers_only(body, name) {
        return None;
    }
    let n_get = collect_pipe_gets(body, name).len();
    let n_contains = count_pipe_contains(body, name);
    // ≥2 lookups (get/contains mix) keep the Hash map.
    if n_get + n_contains != 1 {
        return None;
    }
    Some(rewrite_coll_lookups(
        ctx,
        body,
        name,
        inner,
        span,
        CollLookupKind::Map,
    ))
}

fn try_deforest_toset_let(
    ctx: &LowerCtx,
    name: &str,
    inner: &lumia_syntax::Expr,
    body: &Expr,
    span: Span,
) -> Option<Expr> {
    if !set_lookup_consumers_only(body, name) {
        return None;
    }
    if count_pipe_contains(body, name) != 1 {
        return None;
    }
    Some(rewrite_coll_lookups(
        ctx,
        body,
        name,
        inner,
        span,
        CollLookupKind::Set,
    ))
}

#[derive(Clone, Copy)]
enum CollLookupKind {
    Map,
    Set,
}

fn expr_mentions_var(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Var(n, _) => n == name,
        Expr::Let {
            name: bind,
            value,
            body,
            ..
        } => expr_mentions_var(value, name) || (bind != name && expr_mentions_var(body, name)),
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
            cond, body, step, ..
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

fn collect_pipe_gets(e: &Expr, name: &str) -> Vec<Expr> {
    let mut out = Vec::new();
    collect_pipe_gets_into(e, name, &mut out);
    out
}

fn collect_pipe_gets_into(e: &Expr, name: &str, out: &mut Vec<Expr>) {
    match e {
        Expr::BuiltinCall {
            name: Builtin::ListGet,
            args,
            span,
        } if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            out.push(args.get(1).cloned().unwrap_or(Expr::Int(0, *span)));
        }
        Expr::Let {
            name: bind,
            value,
            body,
            ..
        } => {
            collect_pipe_gets_into(value, name, out);
            if bind != name {
                collect_pipe_gets_into(body, name, out);
            }
        }
        Expr::Assign { value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Return { value, .. } => {
            collect_pipe_gets_into(value, name, out);
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                collect_pipe_gets_into(s, name, out);
            }
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_pipe_gets_into(cond, name, out);
            collect_pipe_gets_into(then_branch, name, out);
            collect_pipe_gets_into(else_branch, name, out);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            collect_pipe_gets_into(cond, name, out);
            collect_pipe_gets_into(body, name, out);
            if let Some(s) = step {
                collect_pipe_gets_into(s, name, out);
            }
        }
        Expr::Call { callee, args, .. } => {
            collect_pipe_gets_into(callee, name, out);
            for a in args {
                collect_pipe_gets_into(a, name, out);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_pipe_gets_into(left, name, out);
            collect_pipe_gets_into(right, name, out);
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                collect_pipe_gets_into(a, name, out);
            }
        }
        _ => {}
    }
}

fn count_pipe_contains(e: &Expr, name: &str) -> usize {
    let mut n = 0usize;
    crate::for_each_expr(e, &mut |x| {
        if let Expr::BuiltinCall {
            name: Builtin::Contains,
            args,
            ..
        } = x
        {
            if args
                .first()
                .is_some_and(|a| matches!(a, Expr::Var(v, _) if v == name))
            {
                n += 1;
            }
        }
    });
    n
}

fn pipe_has_len(e: &Expr, name: &str) -> bool {
    match e {
        Expr::BuiltinCall {
            name: Builtin::ListLen,
            args,
            ..
        } if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            true
        }
        Expr::Let {
            name: bind,
            value,
            body,
            ..
        } => pipe_has_len(value, name) || (bind != name && pipe_has_len(body, name)),
        Expr::Assign { value, .. }
        | Expr::Unary { expr: value, .. }
        | Expr::Return { value, .. } => pipe_has_len(value, name),
        Expr::Seq { stmts, .. } => stmts.iter().any(|s| pipe_has_len(s, name)),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            pipe_has_len(cond, name)
                || pipe_has_len(then_branch, name)
                || pipe_has_len(else_branch, name)
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            pipe_has_len(cond, name)
                || pipe_has_len(body, name)
                || step.as_ref().is_some_and(|s| pipe_has_len(s, name))
        }
        Expr::Call { callee, args, .. } => {
            pipe_has_len(callee, name) || args.iter().any(|a| pipe_has_len(a, name))
        }
        Expr::Binary { left, right, .. } => pipe_has_len(left, name) || pipe_has_len(right, name),
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            args.iter().any(|a| pipe_has_len(a, name))
        }
        _ => false,
    }
}

fn rewrite_shared_gets(
    ctx: &LowerCtx,
    body: &Expr,
    name: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    indices: &[Expr],
    need_len: bool,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let seen = format!("__fuse_seen_{uid}");
    let x0 = format!("__fuse_x_{uid}");
    let x_out = format!("__fuse_xm_{uid}");
    let n = indices.len();
    let accs: Vec<String> = (0..n).map(|i| format!("__get_acc_{uid}_{i}")).collect();
    let idxs: Vec<String> = (0..n).map(|i| format!("__fuse_idx_{uid}_{i}")).collect();
    let gs: Vec<String> = (0..n).map(|i| format!("__get_v_{uid}_{i}")).collect();

    let (cur, mut lets, guards) = stage_pipeline(ctx, stages, &x0, span);
    lets.push((x_out.clone(), cur));

    let mut slot_ifs: Vec<Expr> = Vec::with_capacity(n);
    for i in 0..n {
        slot_ifs.push(Expr::If {
            cond: Box::new(Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(Expr::Var(seen.clone(), span)),
                right: Box::new(Expr::Var(idxs[i].clone(), span)),
                span,
            }),
            then_branch: Box::new(Expr::Assign {
                name: accs[i].clone(),
                value: Box::new(crate::list_hof::option_some(
                    ctx,
                    Expr::Var(x_out.clone(), span),
                    span,
                )),
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        });
    }
    slot_ifs.push(Expr::Assign {
        name: seen.clone(),
        value: Box::new(Expr::Binary {
            op: BinOp::Add,
            left: Box::new(Expr::Var(seen.clone(), span)),
            right: Box::new(Expr::Int(1, span)),
            span,
        }),
        span,
    });
    if !need_len {
        let all = accs
            .iter()
            .map(|a| is_some_var(ctx, a, span))
            .reduce(|a, b| Expr::Binary {
                op: BinOp::And,
                left: Box::new(a),
                right: Box::new(b),
                span,
            })
            .unwrap_or(Expr::Bool(true, span));
        slot_ifs.push(Expr::If {
            cond: Box::new(all),
            then_branch: Box::new(Expr::Break(span)),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        });
    }
    let step = wrap_staged_step(
        Expr::Seq {
            stmts: slot_ifs,
            span,
        },
        lets,
        guards,
        span,
    );
    let source_e = lower_expr(ctx, source);
    let mut nxt = 0usize;
    let rewritten =
        replace_pipe_slots(body, name, &gs, &mut nxt, need_len.then_some(seen.as_str()));
    let mut after = rewritten;
    for (g, acc) in gs.iter().zip(accs.iter()).rev() {
        after = Expr::Let {
            name: g.clone(),
            value: Box::new(option_payload_or_oob_get(
                ctx,
                Expr::Var(acc.clone(), span),
                span,
            )),
            body: Box::new(after),
            mutable: false,
            ty: None,
        };
    }
    let mut binds: Vec<(String, Expr, bool)> = Vec::new();
    for (idx_n, idx_e) in idxs.iter().zip(indices.iter()) {
        binds.push((idx_n.clone(), idx_e.clone(), false));
    }
    binds.push((seen, Expr::Int(0, span), true));
    for acc in &accs {
        binds.push((acc.clone(), crate::list_hof::option_none(ctx, span), true));
    }
    nest_lets(
        binds,
        Expr::Seq {
            stmts: vec![for_each_elem(&x0, source_e, step, span), after],
            span,
        },
    )
}

fn replace_pipe_slots(
    e: &Expr,
    name: &str,
    slots: &[String],
    i: &mut usize,
    len_slot: Option<&str>,
) -> Expr {
    match e {
        Expr::BuiltinCall {
            name: Builtin::ListGet,
            args,
            span: s,
        } if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            let g = slots
                .get(*i)
                .cloned()
                .unwrap_or_else(|| format!("__get_v_{}", *i));
            *i += 1;
            Expr::Var(g, *s)
        }
        Expr::BuiltinCall {
            name: Builtin::ListLen,
            args,
            span: s,
        } if len_slot.is_some()
            && args
                .first()
                .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            Expr::Var(len_slot.unwrap().to_string(), *s)
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
                    value: Box::new(replace_pipe_slots(value, name, slots, i, len_slot)),
                    body: Box::new((**body).clone()),
                    mutable: *mutable,
                    ty: ty.clone(),
                }
            } else {
                Expr::Let {
                    name: bind.clone(),
                    value: Box::new(replace_pipe_slots(value, name, slots, i, len_slot)),
                    body: Box::new(replace_pipe_slots(body, name, slots, i, len_slot)),
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
            value: Box::new(replace_pipe_slots(value, name, slots, i, len_slot)),
            span: *s,
        },
        Expr::Seq { stmts, span: s } => Expr::Seq {
            stmts: stmts
                .iter()
                .map(|st| replace_pipe_slots(st, name, slots, i, len_slot))
                .collect(),
            span: *s,
        },
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span: s,
        } => Expr::If {
            cond: Box::new(replace_pipe_slots(cond, name, slots, i, len_slot)),
            then_branch: Box::new(replace_pipe_slots(then_branch, name, slots, i, len_slot)),
            else_branch: Box::new(replace_pipe_slots(else_branch, name, slots, i, len_slot)),
            span: *s,
        },
        Expr::Loop {
            cond,
            body,
            step,
            span: s,
        } => Expr::Loop {
            cond: Box::new(replace_pipe_slots(cond, name, slots, i, len_slot)),
            body: Box::new(replace_pipe_slots(body, name, slots, i, len_slot)),
            step: step
                .as_ref()
                .map(|st| Box::new(replace_pipe_slots(st, name, slots, i, len_slot))),
            span: *s,
        },
        Expr::Call {
            callee,
            args,
            span: s,
        } => Expr::Call {
            callee: Box::new(replace_pipe_slots(callee, name, slots, i, len_slot)),
            args: args
                .iter()
                .map(|a| replace_pipe_slots(a, name, slots, i, len_slot))
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
            left: Box::new(replace_pipe_slots(left, name, slots, i, len_slot)),
            right: Box::new(replace_pipe_slots(right, name, slots, i, len_slot)),
            span: *s,
        },
        Expr::Unary { op, expr, span: s } => Expr::Unary {
            op: *op,
            expr: Box::new(replace_pipe_slots(expr, name, slots, i, len_slot)),
            span: *s,
        },
        Expr::Return { value, span: s } => Expr::Return {
            value: Box::new(replace_pipe_slots(value, name, slots, i, len_slot)),
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
                .map(|a| replace_pipe_slots(a, name, slots, i, len_slot))
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
                .map(|a| replace_pipe_slots(a, name, slots, i, len_slot))
                .collect(),
            span: *s,
        },
        other => other.clone(),
    }
}

enum PipeCut {
    Take(Expr),
    Drop(Expr),
}

fn peel_pipe_cut(e: &Expr, name: &str) -> Option<PipeCut> {
    match e {
        Expr::BuiltinCall {
            name: b @ (Builtin::ListTake | Builtin::ListSlice),
            args,
            span,
        } if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            let n = args.get(1).cloned().unwrap_or(Expr::Int(0, *span));
            Some(if matches!(b, Builtin::ListTake) {
                PipeCut::Take(n)
            } else {
                PipeCut::Drop(n)
            })
        }
        _ => None,
    }
}

fn fuse_pipe_get_under_cut(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    cut: PipeCut,
    index: Expr,
    span: Span,
) -> Expr {
    match cut {
        PipeCut::Take(n) => {
            let uid = span.start.0;
            let raw = format!("__take_raw_{uid}");
            let lim = format!("__take_n_{uid}");
            let idx = format!("__take_get_idx_{uid}");
            let fused = match fmap {
                Some(f) => fuse_hof_get_flat_map(
                    ctx,
                    source,
                    stages,
                    f,
                    Expr::Var(idx.clone(), span),
                    span,
                ),
                None => fuse_hof_get(ctx, source, stages, Expr::Var(idx.clone(), span), span),
            };
            let body = Expr::If {
                cond: Box::new(Expr::Binary {
                    op: BinOp::Lt,
                    left: Box::new(Expr::Var(idx.clone(), span)),
                    right: Box::new(Expr::Var(lim.clone(), span)),
                    span,
                }),
                then_branch: Box::new(fused),
                else_branch: Box::new(empty_get_oob(span)),
                span,
            };
            bind_nonneg_lim(
                raw,
                lim,
                n,
                Expr::Let {
                    name: idx,
                    value: Box::new(index),
                    body: Box::new(body),
                    mutable: false,
                    ty: None,
                },
                span,
            )
        }
        PipeCut::Drop(n) => {
            let uid = span.start.0;
            let raw = format!("__drop_raw_{uid}");
            let lim = format!("__drop_n_{uid}");
            let adj = Expr::Binary {
                op: BinOp::Add,
                left: Box::new(index),
                right: Box::new(Expr::Var(lim.clone(), span)),
                span,
            };
            let fused = match fmap {
                Some(f) => fuse_hof_get_flat_map(ctx, source, stages, f, adj, span),
                None => fuse_hof_get(ctx, source, stages, adj, span),
            };
            bind_nonneg_lim(raw, lim, n, fused, span)
        }
    }
}

fn fuse_pipe_len_under_cut(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    cut: PipeCut,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    match cut {
        PipeCut::Take(n) => {
            let raw = format!("__take_raw_{uid}");
            let lim = format!("__take_n_{uid}");
            let body = match fmap {
                Some(f) => fuse_hof_len_flat_map_capped(ctx, source, stages, f, &lim, span),
                None => fuse_hof_len_capped(ctx, source, stages, &lim, span),
            };
            bind_nonneg_lim(raw, lim, n, body, span)
        }
        PipeCut::Drop(n) => {
            let raw = format!("__drop_raw_{uid}");
            let lim = format!("__drop_n_{uid}");
            let body = match fmap {
                Some(f) => fuse_hof_len_flat_map_skip(ctx, source, stages, f, &lim, span),
                None => fuse_hof_len_skip(ctx, source, stages, &lim, span),
            };
            bind_nonneg_lim(raw, lim, n, body, span)
        }
    }
}

fn fuse_pipe_contains(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    needle: Expr,
    span: Span,
) -> Expr {
    let nv = format!("__contains_n_{}", span.start.0);
    let p = contains_eq_lambda(&nv, span);
    let search = match fmap {
        Some(f) => fuse_hof_search_flat_map(ctx, source, stages, f, &p, span, FuseSearchKind::Any),
        None => fuse_hof_search(ctx, source, stages, &p, span, FuseSearchKind::Any),
    };
    Expr::Let {
        name: nv,
        value: Box::new(needle),
        body: Box::new(search),
        mutable: false,
        ty: None,
    }
}

fn fuse_pipe_contains_under_cut(
    ctx: &LowerCtx,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    cut: PipeCut,
    needle: Expr,
    span: Span,
) -> Expr {
    let uid = span.start.0;
    let nv = format!("__contains_n_{uid}");
    let p = contains_eq_lambda(&nv, span);
    let search = match cut {
        PipeCut::Take(n) => {
            let raw = format!("__take_raw_{uid}");
            let lim = format!("__take_n_{uid}");
            let body = match fmap {
                Some(f) => fuse_hof_search_flat_map_capped(
                    ctx,
                    source,
                    stages,
                    f,
                    &p,
                    &lim,
                    span,
                    FuseSearchKind::Any,
                ),
                None => {
                    fuse_hof_search_capped(ctx, source, stages, &p, &lim, span, FuseSearchKind::Any)
                }
            };
            bind_nonneg_lim(raw, lim, n, body, span)
        }
        PipeCut::Drop(n) => {
            let raw = format!("__drop_raw_{uid}");
            let lim = format!("__drop_n_{uid}");
            let body = match fmap {
                Some(f) => fuse_hof_search_flat_map_skip(
                    ctx,
                    source,
                    stages,
                    f,
                    &p,
                    &lim,
                    span,
                    FuseSearchKind::Any,
                ),
                None => {
                    fuse_hof_search_skip(ctx, source, stages, &p, &lim, span, FuseSearchKind::Any)
                }
            };
            bind_nonneg_lim(raw, lim, n, body, span)
        }
    };
    Expr::Let {
        name: nv,
        value: Box::new(needle),
        body: Box::new(search),
        mutable: false,
        ty: None,
    }
}

/// Lowered `for x in ys` / `for x in ys.take(n)` — `Elems` + indexed get loop.
fn match_list_for_in(e: &Expr, pipe: &str) -> Option<(String, Expr, Option<PipeCut>)> {
    let Expr::Let {
        name: xs,
        value,
        body,
        ..
    } = e
    else {
        return None;
    };
    let Expr::BuiltinCall {
        name: Builtin::Elems,
        args,
        ..
    } = value.as_ref()
    else {
        return None;
    };
    let list = args.first()?;
    let cut = if matches!(list, Expr::Var(n, _) if n == pipe) {
        None
    } else {
        Some(peel_pipe_cut(list, pipe)?)
    };
    let Expr::Let {
        value: nval,
        body: body2,
        ..
    } = body.as_ref()
    else {
        return None;
    };
    let Expr::BuiltinCall {
        name: Builtin::ListLen,
        args: len_args,
        ..
    } = nval.as_ref()
    else {
        return None;
    };
    if !len_args
        .first()
        .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == xs))
    {
        return None;
    }
    let Expr::Let {
        body: loop_e,
        mutable: true,
        ..
    } = body2.as_ref()
    else {
        return None;
    };
    let Expr::Loop {
        body: loop_body, ..
    } = loop_e.as_ref()
    else {
        return None;
    };
    let Expr::Let {
        name: binding,
        value: get,
        body: user,
        ..
    } = loop_body.as_ref()
    else {
        return None;
    };
    let Expr::BuiltinCall {
        name: Builtin::ListGet,
        args: get_args,
        ..
    } = get.as_ref()
    else {
        return None;
    };
    if !get_args
        .first()
        .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == xs))
    {
        return None;
    }
    Some((binding.clone(), (**user).clone(), cut))
}

fn fuse_pipe_for_in(
    ctx: &LowerCtx,
    binding: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    user: Expr,
    cut: Option<PipeCut>,
    span: Span,
) -> Expr {
    match cut {
        None => match fmap {
            Some(f) => fuse_hof_for_in_flat_map(ctx, binding, source, stages, f, user, span),
            None => fuse_hof_for_in(ctx, binding, source, stages, user, span),
        },
        Some(PipeCut::Take(n)) => {
            let uid = span.start.0;
            let raw = format!("__take_raw_{uid}");
            let lim = format!("__take_n_{uid}");
            let body = match fmap {
                Some(f) => fuse_hof_for_in_flat_map_capped(
                    ctx, binding, source, stages, f, user, &lim, span,
                ),
                None => fuse_hof_for_in_capped(ctx, binding, source, stages, user, &lim, span),
            };
            bind_nonneg_lim(raw, lim, n, body, span)
        }
        Some(PipeCut::Drop(n)) => {
            let uid = span.start.0;
            let raw = format!("__drop_raw_{uid}");
            let lim = format!("__drop_n_{uid}");
            let body = match fmap {
                Some(f) => {
                    fuse_hof_for_in_flat_map_skip(ctx, binding, source, stages, f, user, &lim, span)
                }
                None => fuse_hof_for_in_skip(ctx, binding, source, stages, user, &lim, span),
            };
            bind_nonneg_lim(raw, lim, n, body, span)
        }
    }
}

fn rewrite_pipe_consumers(
    ctx: &LowerCtx,
    e: &Expr,
    name: &str,
    source: &lumia_syntax::Expr,
    stages: &[HofStage<'_>],
    fmap: Option<&lumia_syntax::Expr>,
    span: Span,
) -> Expr {
    if let Some((binding, user, cut)) = match_list_for_in(e, name) {
        return fuse_pipe_for_in(ctx, &binding, source, stages, fmap, user, cut, span);
    }
    if let Expr::BuiltinCall {
        name: Builtin::ListGet,
        args,
        span: s,
    } = e
    {
        if let Some(cut) = args.first().and_then(|a| peel_pipe_cut(a, name)) {
            let idx = args.get(1).cloned().unwrap_or(Expr::Int(0, *s));
            return fuse_pipe_get_under_cut(ctx, source, stages, fmap, cut, idx, *s);
        }
    }
    if let Expr::BuiltinCall {
        name: Builtin::ListLen,
        args,
        span: s,
    } = e
    {
        if let Some(cut) = args.first().and_then(|a| peel_pipe_cut(a, name)) {
            return fuse_pipe_len_under_cut(ctx, source, stages, fmap, cut, *s);
        }
    }
    if let Expr::BuiltinCall {
        name: Builtin::Contains,
        args,
        span: s,
    } = e
    {
        if let Some(cut) = args.first().and_then(|a| peel_pipe_cut(a, name)) {
            let needle = args.get(1).cloned().unwrap_or(Expr::Unit(*s));
            return fuse_pipe_contains_under_cut(ctx, source, stages, fmap, cut, needle, *s);
        }
        if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name))
        {
            let needle = args.get(1).cloned().unwrap_or(Expr::Unit(*s));
            return fuse_pipe_contains(ctx, source, stages, fmap, needle, *s);
        }
    }
    match e {
        Expr::BuiltinCall {
            name: b @ (Builtin::ListGet | Builtin::ListLen | Builtin::ListTake | Builtin::ListSlice),
            args,
            span: s,
        } if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name)) =>
        {
            match b {
                Builtin::ListGet => {
                    let idx = args.get(1).cloned().unwrap_or(Expr::Int(0, *s));
                    match fmap {
                        Some(f) => fuse_hof_get_flat_map(ctx, source, stages, f, idx, *s),
                        None => fuse_hof_get(ctx, source, stages, idx, *s),
                    }
                }
                Builtin::ListLen => match fmap {
                    Some(f) => fuse_hof_len_flat_map(ctx, source, stages, f, *s),
                    None => fuse_hof_len(ctx, source, stages, *s),
                },
                Builtin::ListTake => {
                    let n = args.get(1).cloned().unwrap_or(Expr::Int(0, *s));
                    match fmap {
                        Some(f) => fuse_hof_take_flat_map(ctx, source, stages, f, n, *s),
                        None => fuse_hof_take(ctx, source, stages, n, *s),
                    }
                }
                Builtin::ListSlice => {
                    let n = args.get(1).cloned().unwrap_or(Expr::Int(0, *s));
                    match fmap {
                        Some(f) => fuse_hof_drop_flat_map(ctx, source, stages, f, n, *s),
                        None => fuse_hof_drop(ctx, source, stages, n, *s),
                    }
                }
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
                        ctx, value, name, source, stages, fmap, span,
                    )),
                    body: Box::new((**body).clone()),
                    mutable: *mutable,
                    ty: ty.clone(),
                }
            } else {
                Expr::Let {
                    name: bind.clone(),
                    value: Box::new(rewrite_pipe_consumers(
                        ctx, value, name, source, stages, fmap, span,
                    )),
                    body: Box::new(rewrite_pipe_consumers(
                        ctx, body, name, source, stages, fmap, span,
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
                ctx, value, name, source, stages, fmap, span,
            )),
            span: *s,
        },
        Expr::Seq { stmts, span: s } => Expr::Seq {
            stmts: stmts
                .iter()
                .map(|st| rewrite_pipe_consumers(ctx, st, name, source, stages, fmap, span))
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
                ctx, cond, name, source, stages, fmap, span,
            )),
            then_branch: Box::new(rewrite_pipe_consumers(
                ctx,
                then_branch,
                name,
                source,
                stages,
                fmap,
                span,
            )),
            else_branch: Box::new(rewrite_pipe_consumers(
                ctx,
                else_branch,
                name,
                source,
                stages,
                fmap,
                span,
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
                ctx, cond, name, source, stages, fmap, span,
            )),
            body: Box::new(rewrite_pipe_consumers(
                ctx, body, name, source, stages, fmap, span,
            )),
            step: step.as_ref().map(|st| {
                Box::new(rewrite_pipe_consumers(
                    ctx, st, name, source, stages, fmap, span,
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
                ctx, callee, name, source, stages, fmap, span,
            )),
            args: args
                .iter()
                .map(|a| rewrite_pipe_consumers(ctx, a, name, source, stages, fmap, span))
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
                ctx, left, name, source, stages, fmap, span,
            )),
            right: Box::new(rewrite_pipe_consumers(
                ctx, right, name, source, stages, fmap, span,
            )),
            span: *s,
        },
        Expr::Unary { op, expr, span: s } => Expr::Unary {
            op: *op,
            expr: Box::new(rewrite_pipe_consumers(
                ctx, expr, name, source, stages, fmap, span,
            )),
            span: *s,
        },
        Expr::Return { value, span: s } => Expr::Return {
            value: Box::new(rewrite_pipe_consumers(
                ctx, value, name, source, stages, fmap, span,
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
                .map(|a| rewrite_pipe_consumers(ctx, a, name, source, stages, fmap, span))
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
                .map(|a| rewrite_pipe_consumers(ctx, a, name, source, stages, fmap, span))
                .collect(),
            span: *s,
        },
        other => other.clone(),
    }
}

fn rewrite_coll_lookups(
    ctx: &LowerCtx,
    e: &Expr,
    name: &str,
    inner: &lumia_syntax::Expr,
    span: Span,
    kind: CollLookupKind,
) -> Expr {
    if let Expr::BuiltinCall {
        name: b,
        args,
        span: s,
    } = e
    {
        if args
            .first()
            .is_some_and(|a| matches!(a, Expr::Var(n, _) if n == name))
        {
            let key = args.get(1).cloned().unwrap_or(Expr::Unit(*s));
            match (*b, kind) {
                (Builtin::ListGet, CollLookupKind::Map) => {
                    if let Some(fused) = fuse_tomap_lookup(ctx, inner, key, *s, TomapLookup::Get) {
                        return fused;
                    }
                }
                (Builtin::Contains, CollLookupKind::Map) => {
                    if let Some(fused) =
                        fuse_tomap_lookup(ctx, inner, key, *s, TomapLookup::Contains)
                    {
                        return fused;
                    }
                }
                (Builtin::Contains, CollLookupKind::Set) => {
                    if let Some(fused) = fuse_toset_contains(ctx, inner, key, *s) {
                        return fused;
                    }
                }
                _ => {}
            }
        }
    }
    let rec = |x: &Expr| rewrite_coll_lookups(ctx, x, name, inner, span, kind);
    match e {
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
                    value: Box::new(rec(value)),
                    body: Box::new((**body).clone()),
                    mutable: *mutable,
                    ty: ty.clone(),
                }
            } else {
                Expr::Let {
                    name: bind.clone(),
                    value: Box::new(rec(value)),
                    body: Box::new(rec(body)),
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
            value: Box::new(rec(value)),
            span: *s,
        },
        Expr::Seq { stmts, span: s } => Expr::Seq {
            stmts: stmts.iter().map(rec).collect(),
            span: *s,
        },
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span: s,
        } => Expr::If {
            cond: Box::new(rec(cond)),
            then_branch: Box::new(rec(then_branch)),
            else_branch: Box::new(rec(else_branch)),
            span: *s,
        },
        Expr::Loop {
            cond,
            body,
            step,
            span: s,
        } => Expr::Loop {
            cond: Box::new(rec(cond)),
            body: Box::new(rec(body)),
            step: step.as_ref().map(|st| Box::new(rec(st))),
            span: *s,
        },
        Expr::Call {
            callee,
            args,
            span: s,
        } => Expr::Call {
            callee: Box::new(rec(callee)),
            args: args.iter().map(rec).collect(),
            span: *s,
        },
        Expr::Binary {
            op,
            left,
            right,
            span: s,
        } => Expr::Binary {
            op: *op,
            left: Box::new(rec(left)),
            right: Box::new(rec(right)),
            span: *s,
        },
        Expr::Unary { op, expr, span: s } => Expr::Unary {
            op: *op,
            expr: Box::new(rec(expr)),
            span: *s,
        },
        Expr::Return { value, span: s } => Expr::Return {
            value: Box::new(rec(value)),
            span: *s,
        },
        Expr::BuiltinCall {
            name: b,
            args,
            span: s,
        } => Expr::BuiltinCall {
            name: *b,
            args: args.iter().map(rec).collect(),
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
            args: args.iter().map(rec).collect(),
            span: *s,
        },
        other => other.clone(),
    }
}

//! List map / sortBy desugaring.

use super::{append_assign, list_accum, resolve_unary_callback, with_fun_bind, UnaryCallback};
use crate::ast::{Builtin, Expr};
use crate::lower::{empty_list, LowerCtx};
use crate::visit::free_vars_expr;
use lumi_syntax::Span;

/// `xs.map(f)` → `ListParMap` when FunRef-safe; else sequential accumulate.
/// Type checking may demote `ListParMap` back to sequential (IO / non-scalar).
pub(crate) fn lower_list_map(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    if map_callback_is_parallel_safe(ctx, &f) {
        return Expr::BuiltinCall {
            name: Builtin::ListParMap,
            args: vec![list, f],
            span,
        };
    }
    desugar_list_map_sequential(ctx, list, f, span)
}

/// Sequential `map` loop (also used when auto-parallel demotes `ListParMap`).
pub fn desugar_list_map_sequential(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    match resolve_unary_callback(f, span, "map") {
        UnaryCallback::Inline {
            param,
            param_ty,
            body,
        } => lower_list_map_inline(ctx, list, param, param_ty, body, span),
        UnaryCallback::Bound { f, f_name, x } => lower_list_map_call(ctx, list, f, f_name, x, span),
    }
}

/// Parallel map: capture-free lambda, or a top-level function name (FunRef).
/// Free refs to other top-level funs (e.g. `{ x -> double(x) }`) are FunRef-safe.
fn map_callback_is_parallel_safe(ctx: &LowerCtx, f: &Expr) -> bool {
    match f {
        Expr::Lambda { params, body, .. } => {
            let bound: Vec<String> = params.clone();
            let frees = free_vars_expr(body, &bound);
            frees.iter().all(|n| ctx.is_toplevel_fun(n))
        }
        Expr::Var(n, _) => ctx.is_toplevel_fun(n),
        _ => false,
    }
}

/// `xs.sortBy(f)` — key must be Int / String / Char; stable permute of elements.
pub(crate) fn lower_list_sort_by(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    let xs = format!("__sby_xs_{}", span.start.0);
    let keys = format!("__sby_keys_{}", span.start.0);
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: keys.clone(),
            value: Box::new(lower_list_map(ctx, Expr::Var(xs.clone(), span), f, span)),
            body: Box::new(Expr::BuiltinCall {
                name: Builtin::ListSortByKeys,
                args: vec![Expr::Var(xs, span), Expr::Var(keys, span)],
                span,
            }),
            mutable: false,
            ty: None,
        }),
        mutable: false,
        ty: None,
    }
}

fn lower_list_map_inline(
    ctx: &LowerCtx,
    list: Expr,
    param: String,
    param_ty: Option<String>,
    body: Expr,
    span: Span,
) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
    let x = format!("__map_x_{}", span.start.0);
    let mapped = Expr::Let {
        name: param,
        value: Box::new(Expr::Var(x.clone(), span)),
        body: Box::new(body),
        mutable: false,
        ty: param_ty,
    };
    let step = append_assign(&acc, mapped, span);
    list_accum(ctx, acc, empty_list(span), &x, list, step, span)
}

fn lower_list_map_call(
    ctx: &LowerCtx,
    list: Expr,
    f: Expr,
    f_name: String,
    x: String,
    span: Span,
) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
    let mapped = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(x.clone(), span)],
        span,
    };
    let step = append_assign(&acc, mapped, span);
    with_fun_bind(
        Some((f_name, f)),
        list_accum(ctx, acc, empty_list(span), &x, list, step, span),
    )
}

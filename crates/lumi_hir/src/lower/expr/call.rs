//! Call / method dispatch lowering.

use super::super::collections::{
    lower_set_diff, lower_set_intersect, lower_set_union, lower_to_list, lower_to_map, lower_to_set,
};
use super::super::ctx::LowerCtx;
use super::super::hof_fuse::{
    maybe_fuse_hof_build_filter, maybe_fuse_hof_build_map, maybe_fuse_hof_fold,
};
use super::lower_expr;
use crate::ast::{Builtin, Expr};
use crate::list_hof::{
    lower_list_all, lower_list_any, lower_list_filter, lower_list_find, lower_list_flat_map,
    lower_list_fold, lower_list_for_each, lower_list_map, lower_list_sort_by,
};
use lumi_syntax::{BinOp, Span};

pub(super) fn lower_call(
    ctx: &LowerCtx,
    callee: &lumi_syntax::Expr,
    args: &[lumi_syntax::Expr],
    span: Span,
) -> Expr {
    if let lumi_syntax::Expr::Ident(name, name_span) = callee {
        if name == "fold" && args.len() == 3 {
            if let Some(fused) = maybe_fuse_hof_fold(ctx, &args[0], &args[1], &args[2], span) {
                return fused;
            }
        }
        if name == "map" && args.len() == 2 {
            if let Some(fused) = maybe_fuse_hof_build_map(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "filter" && args.len() == 2 {
            if let Some(fused) = maybe_fuse_hof_build_filter(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        // Free call to a top-level `val`/`foreign` (e.g. `trim(s)`, `println`, `>> trim`):
        // prefer that binding over builtins / method desugar.
        if ctx.is_toplevel_fun(name) {
            // Callee Var must use the ident span — sharing the Call span makes
            // type_at record Fun then Unit on the same range (inlay/hover noise).
            return Expr::Call {
                callee: Box::new(Expr::Var(name.clone(), *name_span)),
                args: args.iter().map(|e| lower_expr(ctx, e)).collect(),
                span,
            };
        }
    }
    // Method call: fuse `….map(…).filter(…).fold(z, g)` on the syntax tree.
    if let lumi_syntax::Expr::Field { base, field, .. } = callee {
        if field == "fold" && args.len() == 2 {
            if let Some(fused) = maybe_fuse_hof_fold(ctx, base, &args[0], &args[1], span) {
                return fused;
            }
        }
        if field == "map" && args.len() == 1 {
            if let Some(fused) = maybe_fuse_hof_build_map(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "filter" && args.len() == 1 {
            if let Some(fused) = maybe_fuse_hof_build_filter(ctx, base, &args[0], span) {
                return fused;
            }
        }
        // `x.show()` → Show builtin (codegen / instance override).
        if field == "show" && args.is_empty() {
            return Expr::BuiltinCall {
                name: Builtin::Show,
                args: vec![lower_expr(ctx, base)],
                span,
            };
        }
        // `x.eq(y)` / `x.less(y)` → same Binary path as `==` / `<` (trait overrides).
        if field == "eq" && args.len() == 1 {
            return Expr::Binary {
                op: lumi_syntax::BinOp::Eq,
                left: Box::new(lower_expr(ctx, base)),
                right: Box::new(lower_expr(ctx, &args[0])),
                span,
            };
        }
        if field == "less" && args.len() == 1 {
            return Expr::Binary {
                op: lumi_syntax::BinOp::Lt,
                left: Box::new(lower_expr(ctx, base)),
                right: Box::new(lower_expr(ctx, &args[0])),
                span,
            };
        }
        let mut call_args = vec![lower_expr(ctx, base)];
        call_args.extend(args.iter().map(|e| lower_expr(ctx, e)));
        return lower_call_from_parts(ctx, Expr::Var(field.clone(), span), call_args, span);
    }
    lower_call_from_parts(
        ctx,
        lower_expr(ctx, callee),
        args.iter().map(|e| lower_expr(ctx, e)).collect(),
        span,
    )
}

fn take2(args: Vec<Expr>) -> (Expr, Expr) {
    let mut it = args.into_iter();
    (it.next().expect("arity"), it.next().expect("arity"))
}

fn take3(args: Vec<Expr>) -> (Expr, Expr, Expr) {
    let mut it = args.into_iter();
    (
        it.next().expect("arity"),
        it.next().expect("arity"),
        it.next().expect("arity"),
    )
}

/// Methods that desugar to loops / collections (not a single BuiltinCall).
fn lower_desugar_method(ctx: &LowerCtx, name: &str, args: Vec<Expr>, span: Span) -> Option<Expr> {
    Some(match (name, args.len()) {
        ("map", 2) => {
            let (xs, f) = take2(args);
            lower_list_map(ctx, xs, f, span)
        }
        ("filter", 2) => {
            let (xs, f) = take2(args);
            lower_list_filter(ctx, xs, f, span)
        }
        ("flatMap", 2) => {
            let (xs, f) = take2(args);
            lower_list_flat_map(ctx, xs, f, span)
        }
        ("fold", 3) => {
            let (xs, z, f) = take3(args);
            lower_list_fold(ctx, xs, z, f, span)
        }
        ("any", 2) => {
            let (xs, f) = take2(args);
            lower_list_any(ctx, xs, f, span)
        }
        ("all", 2) => {
            let (xs, f) = take2(args);
            lower_list_all(ctx, xs, f, span)
        }
        ("find", 2) => {
            let (xs, f) = take2(args);
            lower_list_find(ctx, xs, f, span)
        }
        ("forEach", 2) => {
            let (xs, f) = take2(args);
            lower_list_for_each(ctx, xs, f, span)
        }
        ("sortBy", 2) => {
            let (xs, f) = take2(args);
            lower_list_sort_by(ctx, xs, f, span)
        }
        ("isEmpty", 1) => Expr::Binary {
            op: BinOp::Eq,
            left: Box::new(Expr::BuiltinCall {
                name: Builtin::ListLen,
                args,
                span,
            }),
            right: Box::new(Expr::Int(0, span)),
            span,
        },
        ("toSet", 1) => lower_to_set(ctx, args.into_iter().next().expect("arity"), span),
        ("toList", 1) => lower_to_list(ctx, args.into_iter().next().expect("arity"), span),
        ("toMap", 1) => lower_to_map(ctx, args.into_iter().next().expect("arity"), span),
        ("union", 2) => {
            let (a, b) = take2(args);
            lower_set_union(ctx, a, b, span)
        }
        ("intersect", 2) => {
            let (a, b) = take2(args);
            lower_set_intersect(ctx, a, b, span)
        }
        ("diff", 2) => {
            let (a, b) = take2(args);
            lower_set_diff(ctx, a, b, span)
        }
        ("lines", 1) => Expr::BuiltinCall {
            name: Builtin::StrSplit,
            args: vec![
                args.into_iter().next().expect("arity"),
                Expr::Char('\n', span),
            ],
            span,
        },
        _ => return None,
    })
}

fn flatten_map_of_pairs(args: Vec<Expr>) -> Vec<Expr> {
    let mut flat = Vec::with_capacity(args.len() * 2);
    for a in args {
        if let Expr::Call {
            callee: inner,
            args: kv,
            ..
        } = &a
        {
            if let Expr::Var(n, _) = inner.as_ref() {
                if n == "to" && kv.len() == 2 {
                    flat.push(kv[0].clone());
                    flat.push(kv[1].clone());
                    continue;
                }
            }
        }
        flat.push(a);
    }
    flat
}

pub(super) fn lower_call_from_parts(
    ctx: &LowerCtx,
    callee: Expr,
    args: Vec<Expr>,
    span: Span,
) -> Expr {
    if let Expr::Var(name, _) = &callee {
        if let Some(c) = ctx.lookup_ctor(name) {
            if args.len() == c.arity {
                return Expr::AdtNew {
                    adt_name: c.adt_name,
                    variant: name.clone(),
                    tag: c.tag,
                    args,
                    span,
                };
            }
        }
        if let Some(b) = Builtin::from_intrinsic(name, args.len()) {
            return Expr::BuiltinCall {
                name: b,
                args,
                span,
            };
        }
        if let Some(b) = Builtin::from_method(name, args.len()) {
            return Expr::BuiltinCall {
                name: b,
                args,
                span,
            };
        }
        if let Some(e) = lower_desugar_method(ctx, name, args.clone(), span) {
            return e;
        }
        if name == "mapOf" {
            return Expr::Call {
                callee: Box::new(callee),
                args: flatten_map_of_pairs(args),
                span,
            };
        }
    }
    Expr::Call {
        callee: Box::new(callee),
        args,
        span,
    }
}

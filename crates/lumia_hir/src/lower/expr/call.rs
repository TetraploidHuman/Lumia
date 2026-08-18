//! Call / method dispatch lowering.

use super::super::collections::{
    lower_set_diff, lower_set_intersect, lower_set_union, lower_to_list, lower_to_map, lower_to_set,
};
use super::super::ctx::LowerCtx;
use super::super::hof_fuse::{
    try_fuse_hof_all, try_fuse_hof_any, try_fuse_hof_build_filter, try_fuse_hof_build_map,
    try_fuse_hof_contains, try_fuse_hof_drop, try_fuse_hof_find, try_fuse_hof_flat_map,
    try_fuse_hof_fold, try_fuse_hof_get, try_fuse_hof_is_empty, try_fuse_hof_len,
    try_fuse_hof_take, try_fuse_hof_to_list, try_fuse_hof_to_map, try_fuse_hof_to_set,
};
use super::lower_expr;
use crate::ast::{Builtin, Expr};
use crate::list_hof::{
    lower_list_all, lower_list_any, lower_list_filter, lower_list_find, lower_list_flat_map,
    lower_list_fold, lower_list_map, lower_list_sort_by,
};
use lumia_syntax::{BinOp, Span};

pub(super) fn lower_call(
    ctx: &LowerCtx,
    callee: &lumia_syntax::Expr,
    args: &[lumia_syntax::Expr],
    span: Span,
) -> Expr {
    if let lumia_syntax::Expr::Ident(name, name_span) = callee {
        if name == "println" {
            return Expr::BuiltinCall {
                name: Builtin::Println,
                args: args.iter().map(|e| lower_expr(ctx, e)).collect(),
                span,
            };
        }
        if name == "channel" && args.len() == 1 {
            return Expr::BuiltinCall {
                name: Builtin::ChannelNew,
                args: args.iter().map(|e| lower_expr(ctx, e)).collect(),
                span,
            };
        }
        if name == "cancelScope" && args.is_empty() {
            return Expr::BuiltinCall {
                name: Builtin::ScopeCancel,
                args: vec![],
                span,
            };
        }
        if name == "assert" {
            return Expr::BuiltinCall {
                name: Builtin::Assert,
                args: args.iter().map(|e| lower_expr(ctx, e)).collect(),
                span,
            };
        }
        if name == "readStdin" {
            return Expr::BuiltinCall {
                name: Builtin::ReadStdin,
                args: args.iter().map(|e| lower_expr(ctx, e)).collect(),
                span,
            };
        }
        if name == "fold" && args.len() == 3 {
            if let Some(fused) = try_fuse_hof_fold(ctx, &args[0], &args[1], &args[2], span) {
                return fused;
            }
        }
        if name == "map" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_build_map(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "filter" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_build_filter(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "flatMap" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_flat_map(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "any" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_any(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "all" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_all(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "find" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_find(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "len" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_len(ctx, &args[0], span) {
                return fused;
            }
        }
        if name == "isEmpty" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_is_empty(ctx, &args[0], span) {
                return fused;
            }
        }
        if name == "get" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_get(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "take" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_take(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if (name == "drop" || name == "slice") && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_drop(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "contains" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_contains(ctx, &args[0], &args[1], span) {
                return fused;
            }
        }
        if name == "toList" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_to_list(ctx, &args[0], span) {
                return fused;
            }
        }
        if name == "toSet" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_to_set(ctx, &args[0], span) {
                return fused;
            }
        }
        if name == "toMap" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_to_map(ctx, &args[0], span) {
                return fused;
            }
        }
        // Free call to a top-level `val`/`foreign` (e.g. `trim(s)`, `>> trim`):
        // prefer that binding over `Builtin::from_method`. Method calls like
        // `s.trim()` still desugar through `lower_call_from_parts` → builtin,
        // so `val len = { xs -> xs.len() }` stays non-recursive.
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
    if let lumia_syntax::Expr::Field { base, field, .. } = callee {
        if field == "fold" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_fold(ctx, base, &args[0], &args[1], span) {
                return fused;
            }
        }
        // Fuse `….map/filter….map/filter` before lowering the base (avoids
        // materializing intermediate lists).
        if field == "map" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_build_map(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "filter" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_build_filter(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "flatMap" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_flat_map(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "any" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_any(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "all" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_all(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "find" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_find(ctx, base, &args[0], span) {
                return fused;
            }
        }
        // Fuse `….map/filter….len()` / `….isEmpty()` before materializing lists.
        if field == "len" && args.is_empty() {
            if let Some(fused) = try_fuse_hof_len(ctx, base, span) {
                return fused;
            }
        }
        if field == "isEmpty" && args.is_empty() {
            if let Some(fused) = try_fuse_hof_is_empty(ctx, base, span) {
                return fused;
            }
        }
        if field == "get" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_get(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "take" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_take(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if (field == "drop" || field == "slice") && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_drop(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "contains" && args.len() == 1 {
            if let Some(fused) = try_fuse_hof_contains(ctx, base, &args[0], span) {
                return fused;
            }
        }
        if field == "toList" && args.is_empty() {
            if let Some(fused) = try_fuse_hof_to_list(ctx, base, span) {
                return fused;
            }
        }
        if field == "toSet" && args.is_empty() {
            if let Some(fused) = try_fuse_hof_to_set(ctx, base, span) {
                return fused;
            }
        }
        if field == "toMap" && args.is_empty() {
            if let Some(fused) = try_fuse_hof_to_map(ctx, base, span) {
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
                op: lumia_syntax::BinOp::Eq,
                left: Box::new(lower_expr(ctx, base)),
                right: Box::new(lower_expr(ctx, &args[0])),
                span,
            };
        }
        if field == "less" && args.len() == 1 {
            return Expr::Binary {
                op: lumia_syntax::BinOp::Lt,
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

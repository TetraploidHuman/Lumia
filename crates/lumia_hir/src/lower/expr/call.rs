//! Call / method dispatch lowering.

use super::super::collections::{
    lower_set_diff, lower_set_intersect, lower_set_union, lower_to_list, lower_to_map, lower_to_set,
};
use super::super::ctx::LowerCtx;
use super::super::hof_fuse::try_fuse_hof_fold;
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
    if let lumia_syntax::Expr::Ident(name, _) = callee {
        if name == "println" {
            return Expr::BuiltinCall {
                name: Builtin::Println,
                args: args.iter().map(|e| lower_expr(ctx, e)).collect(),
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
        if name == "fold" && args.len() == 3 {
            if let Some(fused) = try_fuse_hof_fold(ctx, &args[0], &args[1], &args[2], span) {
                return fused;
            }
        }
    }
    // Method call: fuse `….map(…).filter(…).fold(z, g)` on the syntax tree.
    if let lumia_syntax::Expr::Field { base, field, .. } = callee {
        if field == "fold" && args.len() == 2 {
            if let Some(fused) = try_fuse_hof_fold(ctx, base, &args[0], &args[1], span) {
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
        match name.as_str() {
            "len" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListLen,
                    args,
                    span,
                };
            }
            "get" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListGet,
                    args,
                    span,
                };
            }
            "map" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_list_map(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "filter" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_list_filter(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "flatMap" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_list_flat_map(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "fold" if args.len() == 3 => {
                let mut it = args.into_iter();
                return lower_list_fold(
                    ctx,
                    it.next().unwrap(),
                    it.next().unwrap(),
                    it.next().unwrap(),
                    span,
                );
            }
            "any" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_list_any(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "all" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_list_all(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "find" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_list_find(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "append" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListAppend,
                    args,
                    span,
                };
            }
            "isEmpty" if args.len() == 1 => {
                return Expr::Binary {
                    op: BinOp::Eq,
                    left: Box::new(Expr::BuiltinCall {
                        name: Builtin::ListLen,
                        args,
                        span,
                    }),
                    right: Box::new(Expr::Int(0, span)),
                    span,
                };
            }
            "toSet" if args.len() == 1 => {
                return lower_to_set(ctx, args.into_iter().next().unwrap(), span);
            }
            "toList" if args.len() == 1 => {
                return lower_to_list(ctx, args.into_iter().next().unwrap(), span);
            }
            "toMap" if args.len() == 1 => {
                return lower_to_map(ctx, args.into_iter().next().unwrap(), span);
            }
            "union" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_set_union(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "intersect" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_set_intersect(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "diff" if args.len() == 2 => {
                let mut it = args.into_iter();
                return lower_set_diff(ctx, it.next().unwrap(), it.next().unwrap(), span);
            }
            "contains" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::Contains,
                    args,
                    span,
                };
            }
            "set" if args.len() == 3 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapSet,
                    args,
                    span,
                };
            }
            "remove" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapRemove,
                    args,
                    span,
                };
            }
            "insert" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::SetInsert,
                    args,
                    span,
                };
            }
            "keys" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapKeys,
                    args,
                    span,
                };
            }
            "values" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapValues,
                    args,
                    span,
                };
            }
            "items" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::MapItems,
                    args,
                    span,
                };
            }
            "slice" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args,
                    span,
                };
            }
            "drop" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args,
                    span,
                };
            }
            "take" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListTake,
                    args,
                    span,
                };
            }
            "reverse" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListReverse,
                    args,
                    span,
                };
            }
            "sort" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListSort,
                    args,
                    span,
                };
            }
            "sortBy" if args.len() == 2 => {
                return lower_list_sort_by(ctx, args[0].clone(), args[1].clone(), span);
            }
            "join" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListJoin,
                    args,
                    span,
                };
            }
            "lines" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrSplit,
                    args: vec![args[0].clone(), Expr::Char('\n', span)],
                    span,
                };
            }
            "trim" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrTrim,
                    args,
                    span,
                };
            }
            "split" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrSplit,
                    args,
                    span,
                };
            }
            "substring" if args.len() == 3 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrSubstring,
                    args,
                    span,
                };
            }
            "toLower" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrToLower,
                    args,
                    span,
                };
            }
            "toUpper" if args.len() == 1 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrToUpper,
                    args,
                    span,
                };
            }
            "startsWith" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrStartsWith,
                    args,
                    span,
                };
            }
            "endsWith" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::StrEndsWith,
                    args,
                    span,
                };
            }
            "readStdin" if args.is_empty() => {
                return Expr::BuiltinCall {
                    name: Builtin::ReadStdin,
                    args,
                    span,
                };
            }
            "concat" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::ListConcat,
                    args,
                    span,
                };
            }
            "range" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::Range,
                    args,
                    span,
                };
            }
            "rangeInclusive" if args.len() == 2 => {
                return Expr::BuiltinCall {
                    name: Builtin::RangeInclusive,
                    args,
                    span,
                };
            }
            "mapOf" => {
                // Flatten `k to v` → k, v for AllocMap flat layout
                let mut flat = vec![];
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
                return Expr::Call {
                    callee: Box::new(callee),
                    args: flat,
                    span,
                };
            }
            _ => {}
        }
    }
    Expr::Call {
        callee: Box::new(callee),
        args,
        span,
    }
}

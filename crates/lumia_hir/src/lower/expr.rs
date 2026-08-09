//! Expression lowering.

use super::collections::{
    lower_set_diff, lower_set_intersect, lower_set_union, lower_to_list, lower_to_map, lower_to_set,
};
use super::ctx::LowerCtx;
use super::for_loops::lower_for_in;
use super::hof_fuse::try_fuse_hof_fold;
use super::match_arms::{lower_match, lower_match_cond};
use crate::ast::{Builtin, Expr, Fun, Item};
use crate::list_hof::{
    lower_list_all, lower_list_any, lower_list_filter, lower_list_find, lower_list_flat_map,
    lower_list_fold, lower_list_map, lower_list_sort_by,
};
use crate::match_check::{pattern_cond, pattern_irrefutable};
use lumia_syntax::{BinOp, Span};
use std::collections::HashMap;

pub(crate) fn push_lowered_val(
    ctx: &LowerCtx,
    items: &mut Vec<Item>,
    v: &lumia_syntax::ValItem,
    name: &str,
) {
    let body = lower_expr(ctx, &v.body);
    let body = if let Some(params) = &v.params {
        Expr::Lambda {
            params: params.clone(),
            body: Box::new(body),
            span: v.span,
        }
    } else {
        body
    };
    match body {
        Expr::Lambda {
            params,
            body,
            span: _,
        } => {
            items.push(Item::Fun(Fun {
                name: name.to_string(),
                params,
                body: *body,
                is_main: name == "main",
                external: None,
                foreign_sig: None,
                foreign_pure: false,
            }));
        }
        other => {
            if name == "main" {
                items.push(Item::Fun(Fun {
                    name: "main".into(),
                    params: vec![],
                    body: other,
                    is_main: true,
                    external: None,
                    foreign_sig: None,
                    foreign_pure: false,
                }));
            } else {
                items.push(Item::Val {
                    name: name.to_string(),
                    body: other,
                });
            }
        }
    }
}
pub(crate) fn lower_expr(ctx: &LowerCtx, e: &lumia_syntax::Expr) -> Expr {
    match e {
        lumia_syntax::Expr::Int(n, s) => Expr::Int(*n, *s),
        lumia_syntax::Expr::Float(n, s) => Expr::Float(*n, *s),
        lumia_syntax::Expr::Bool(b, s) => Expr::Bool(*b, *s),
        lumia_syntax::Expr::String(t, s) => Expr::String(t.clone(), *s),
        lumia_syntax::Expr::Interp { parts, span } => lower_interp(ctx, parts, *span),
        lumia_syntax::Expr::Char(c, s) => Expr::Char(*c, *s),
        lumia_syntax::Expr::Ident(n, s) => {
            if let Some(c) = ctx.lookup_ctor(n) {
                if c.arity == 0 {
                    return Expr::AdtNew {
                        adt_name: c.adt_name,
                        variant: n.clone(),
                        tag: c.tag,
                        args: vec![],
                        span: *s,
                    };
                }
            }
            Expr::Var(n.clone(), *s)
        }
        lumia_syntax::Expr::Block { stmts, tail, span } => {
            lower_block(ctx, stmts, tail.as_deref(), *span)
        }
        lumia_syntax::Expr::Lambda { params, body, span } => Expr::Lambda {
            params: params.clone(),
            body: Box::new(lower_expr(ctx, body)),
            span: *span,
        },
        lumia_syntax::Expr::Call { callee, args, span } => lower_call(ctx, callee, args, *span),
        lumia_syntax::Expr::Binary {
            op,
            left,
            right,
            span,
        } => {
            // DESIGN: `and` / `or` short-circuit — desugar to `if`.
            let l = lower_expr(ctx, left);
            let r = lower_expr(ctx, right);
            match op {
                BinOp::And => Expr::If {
                    cond: Box::new(l),
                    then_branch: Box::new(r),
                    else_branch: Box::new(Expr::Bool(false, *span)),
                    span: *span,
                },
                BinOp::Or => Expr::If {
                    cond: Box::new(l),
                    then_branch: Box::new(Expr::Bool(true, *span)),
                    else_branch: Box::new(r),
                    span: *span,
                },
                _ => Expr::Binary {
                    op: *op,
                    left: Box::new(l),
                    right: Box::new(r),
                    span: *span,
                },
            }
        }
        lumia_syntax::Expr::Unary { op, expr, span } => Expr::Unary {
            op: *op,
            expr: Box::new(lower_expr(ctx, expr)),
            span: *span,
        },
        lumia_syntax::Expr::If {
            cond,
            then_branch,
            else_branch,
            span,
        } => Expr::If {
            cond: Box::new(lower_expr(ctx, cond)),
            then_branch: Box::new(lower_expr(ctx, then_branch)),
            else_branch: Box::new(
                else_branch
                    .as_ref()
                    .map(|e| lower_expr(ctx, e))
                    .unwrap_or(Expr::Unit(*span)),
            ),
            span: *span,
        },
        lumia_syntax::Expr::Pipeline { left, right, span } => {
            // Fuse `xs >> map … >> filter … >> fold(z, g)` before expanding intermediates.
            if let lumia_syntax::Expr::Call { callee, args, .. } = right.as_ref() {
                if let lumia_syntax::Expr::Ident(name, _) = callee.as_ref() {
                    if name == "fold" && args.len() == 2 {
                        if let Some(fused) = try_fuse_hof_fold(ctx, left, &args[0], &args[1], *span)
                        {
                            return fused;
                        }
                    }
                }
            }
            match right.as_ref() {
                lumia_syntax::Expr::Call { callee, args, .. } => {
                    let mut new_args = vec![lower_expr(ctx, left)];
                    new_args.extend(args.iter().map(|e| lower_expr(ctx, e)));
                    lower_call_from_parts(ctx, lower_expr(ctx, callee), new_args, *span)
                }
                other => lower_call_from_parts(
                    ctx,
                    lower_expr(ctx, other),
                    vec![lower_expr(ctx, left)],
                    *span,
                ),
            }
        }
        lumia_syntax::Expr::Field { base, field, span } => {
            // `xs.len` → len(xs); product fields → adt_field; `p.0` → adt_field;
            // else call field(base)
            if field == "len" {
                Expr::BuiltinCall {
                    name: Builtin::ListLen,
                    args: vec![lower_expr(ctx, base)],
                    span: *span,
                }
            } else if let Ok(idx) = field.parse::<i64>() {
                // Tuple / positional projection (DESIGN: `p.0`)
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![lower_expr(ctx, base), Expr::Int(idx, *span)],
                    span: *span,
                }
            } else if ctx.is_ambiguous_product_field(field) {
                ctx.set_err(
                    format!("cannot resolve field `{field}` (ambiguous across product types)"),
                    *span,
                );
                Expr::Unit(*span)
            } else if let Some((adt_name, idx)) = ctx.lookup_product_field(field) {
                // Carry expected product name so ty can reject wrong receivers
                // (global name→index alone is unsound across distinct products).
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        lower_expr(ctx, base),
                        Expr::Int(idx as i64, *span),
                        Expr::String(adt_name, *span),
                    ],
                    span: *span,
                }
            } else {
                Expr::Call {
                    callee: Box::new(Expr::Var(field.clone(), *span)),
                    args: vec![lower_expr(ctx, base)],
                    span: *span,
                }
            }
        }
        lumia_syntax::Expr::StructLit { name, fields, span } => {
            lower_struct_lit(ctx, name, fields, *span)
        }
        lumia_syntax::Expr::With { base, fields, span } => lower_with(ctx, base, fields, *span),
        lumia_syntax::Expr::TupleLit { elems, span } => Expr::AdtNew {
            adt_name: "__Tuple".into(),
            variant: String::new(),
            tag: 0,
            args: elems.iter().map(|e| lower_expr(ctx, e)).collect(),
            span: *span,
        },
        lumia_syntax::Expr::ListLit { elems, span } => Expr::Call {
            callee: Box::new(Expr::Var("listOf".into(), *span)),
            args: elems.iter().map(|e| lower_expr(ctx, e)).collect(),
            span: *span,
        },
        lumia_syntax::Expr::Match {
            scrutinee,
            arms,
            span,
        } => lower_match(ctx, scrutinee, arms, *span),
        lumia_syntax::Expr::MatchCond { arms, span } => lower_match_cond(ctx, arms, *span),
        lumia_syntax::Expr::Return { value, span } => Expr::Return {
            value: Box::new(lower_expr(ctx, value)),
            span: *span,
        },
        lumia_syntax::Expr::Alt {
            scrutinee,
            alt,
            span,
        } => Expr::Alt {
            scrutinee: Box::new(lower_expr(ctx, scrutinee)),
            alt: Box::new(lower_expr(ctx, alt)),
            span: *span,
        },
    }
}

fn lower_call(
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

fn lower_call_from_parts(ctx: &LowerCtx, callee: Expr, args: Vec<Expr>, span: Span) -> Expr {
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
                return lower_list_map(ctx, args[0].clone(), args[1].clone(), span);
            }
            "filter" if args.len() == 2 => {
                return lower_list_filter(ctx, args[0].clone(), args[1].clone(), span);
            }
            "flatMap" if args.len() == 2 => {
                return lower_list_flat_map(ctx, args[0].clone(), args[1].clone(), span);
            }
            "fold" if args.len() == 3 => {
                return lower_list_fold(
                    ctx,
                    args[0].clone(),
                    args[1].clone(),
                    args[2].clone(),
                    span,
                );
            }
            "any" if args.len() == 2 => {
                return lower_list_any(ctx, args[0].clone(), args[1].clone(), span);
            }
            "all" if args.len() == 2 => {
                return lower_list_all(ctx, args[0].clone(), args[1].clone(), span);
            }
            "find" if args.len() == 2 => {
                return lower_list_find(ctx, args[0].clone(), args[1].clone(), span);
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
                        args: vec![args[0].clone()],
                        span,
                    }),
                    right: Box::new(Expr::Int(0, span)),
                    span,
                };
            }
            "toSet" if args.len() == 1 => {
                return lower_to_set(ctx, args[0].clone(), span);
            }
            "toList" if args.len() == 1 => {
                return lower_to_list(ctx, args[0].clone(), span);
            }
            "toMap" if args.len() == 1 => {
                return lower_to_map(ctx, args[0].clone(), span);
            }
            "union" if args.len() == 2 => {
                return lower_set_union(ctx, args[0].clone(), args[1].clone(), span);
            }
            "intersect" if args.len() == 2 => {
                return lower_set_intersect(ctx, args[0].clone(), args[1].clone(), span);
            }
            "diff" if args.len() == 2 => {
                return lower_set_diff(ctx, args[0].clone(), args[1].clone(), span);
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

fn lower_struct_lit(
    ctx: &LowerCtx,
    name: &str,
    fields: &[(String, lumia_syntax::Expr)],
    span: Span,
) -> Expr {
    let Some(order) = ctx.lookup_product(name) else {
        // Unknown product — leave as call-shaped fallback
        return Expr::Call {
            callee: Box::new(Expr::Var(name.into(), span)),
            args: fields.iter().map(|(_, e)| lower_expr(ctx, e)).collect(),
            span,
        };
    };
    let mut by_name: HashMap<String, Expr> = HashMap::new();
    for (f, e) in fields {
        if by_name.insert(f.clone(), lower_expr(ctx, e)).is_some() {
            ctx.set_err(
                format!("duplicate field `{f}` in `{name}` struct literal"),
                span,
            );
        }
    }
    let mut args = Vec::with_capacity(order.len());
    for f in &order {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            ctx.set_err(
                format!("missing field `{f}` in `{name}` struct literal"),
                span,
            );
            // Placeholder; `lower_module` aborts on LOWER_ERR.
            args.push(Expr::Int(0, span));
        }
    }
    if let Some((extra, _)) = by_name.iter().next() {
        ctx.set_err(
            format!("unknown field `{extra}` in `{name}` struct literal"),
            span,
        );
    }
    Expr::AdtNew {
        adt_name: name.into(),
        variant: name.into(),
        tag: 0,
        args,
        span,
    }
}

fn lower_with(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    fields: &[(String, lumia_syntax::Expr)],
    span: Span,
) -> Expr {
    // Infer product from first updated field name. Shared field names across
    // product types are stripped from the map (ambiguous) and must error here.
    let Some((fname, _)) = fields.first() else {
        return lower_expr(ctx, base);
    };
    let Some((type_name, _)) = ctx.lookup_product_field(fname) else {
        ctx.set_err(
            format!(
                "cannot resolve `with` field `{fname}` (unknown or ambiguous across product types)"
            ),
            span,
        );
        return lower_expr(ctx, base);
    };
    let Some(order) = ctx.lookup_product(&type_name) else {
        ctx.set_err(
            format!("unknown product type `{type_name}` in `with`"),
            span,
        );
        return lower_expr(ctx, base);
    };
    let base_e = lower_expr(ctx, base);
    // Bind base once
    let tmp = format!("__with_{}", span.start.0);
    let mut by_name: HashMap<String, Expr> = HashMap::new();
    for (f, e) in fields {
        by_name.insert(f.clone(), lower_expr(ctx, e));
    }
    let mut args = Vec::with_capacity(order.len());
    for (i, f) in order.iter().enumerate() {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            args.push(Expr::BuiltinCall {
                name: Builtin::AdtField,
                args: vec![Expr::Var(tmp.clone(), span), Expr::Int(i as i64, span)],
                span,
            });
        }
    }
    Expr::Let {
        name: tmp,
        value: Box::new(base_e),
        body: Box::new(Expr::AdtNew {
            adt_name: type_name,
            variant: String::new(),
            tag: 0,
            args,
            span,
        }),
        mutable: false,
    }
}

fn lower_block(
    ctx: &LowerCtx,
    stmts: &[lumia_syntax::Stmt],
    tail: Option<&lumia_syntax::Expr>,
    span: Span,
) -> Expr {
    fn fold(
        ctx: &LowerCtx,
        stmts: &[lumia_syntax::Stmt],
        tail: Option<&lumia_syntax::Expr>,
        span: Span,
    ) -> Expr {
        if stmts.is_empty() {
            return match tail {
                Some(e) => lower_expr(ctx, e),
                None => Expr::Unit(span),
            };
        }
        let (first, rest) = stmts.split_first().unwrap();
        match first {
            lumia_syntax::Stmt::Val { pat, expr, span: s } => {
                lower_val_pat(ctx, pat, expr, *s, fold(ctx, rest, tail, span))
            }
            lumia_syntax::Stmt::Var {
                name,
                expr,
                span: s,
            } => {
                let _ = s;
                Expr::Let {
                    name: name.clone(),
                    value: Box::new(lower_expr(ctx, expr)),
                    body: Box::new(fold(ctx, rest, tail, span)),
                    mutable: true,
                }
            }
            lumia_syntax::Stmt::Assign {
                name,
                expr,
                span: s,
            } => {
                let assign = Expr::Assign {
                    name: name.clone(),
                    value: Box::new(lower_expr(ctx, expr)),
                    span: *s,
                };
                let rest_e = fold(ctx, rest, tail, span);
                Expr::Seq {
                    stmts: vec![assign, rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::Expr(e) => {
                let e = lower_expr(ctx, e);
                let rest_e = fold(ctx, rest, tail, span);
                Expr::Seq {
                    stmts: vec![e, rest_e],
                    span,
                }
            }
            lumia_syntax::Stmt::Break(s) => {
                let rest_e = fold(ctx, rest, tail, span);
                Expr::Seq {
                    stmts: vec![Expr::Break(*s), rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::Continue(s) => {
                let rest_e = fold(ctx, rest, tail, span);
                Expr::Seq {
                    stmts: vec![Expr::Continue(*s), rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::ForCond {
                cond,
                body,
                span: s,
            } => {
                let loop_e = Expr::Loop {
                    cond: Box::new(lower_expr(ctx, cond)),
                    body: Box::new(lower_expr(ctx, body)),
                    step: None,
                    span: *s,
                };
                let rest_e = fold(ctx, rest, tail, span);
                Expr::Seq {
                    stmts: vec![loop_e, rest_e],
                    span: *s,
                }
            }
            lumia_syntax::Stmt::ForIn {
                binding,
                iter,
                body,
                span: s,
            } => {
                let loop_e = lower_for_in(ctx, binding, iter, body, *s);
                let rest_e = fold(ctx, rest, tail, span);
                Expr::Seq {
                    stmts: vec![loop_e, rest_e],
                    span: *s,
                }
            }
        }
    }
    fold(ctx, stmts, tail, span)
}

/// `val pat = e` — irrefutable pattern bindings (tuple / product / binder).
fn lower_val_pat(
    ctx: &LowerCtx,
    pat: &lumia_syntax::Pattern,
    expr: &lumia_syntax::Expr,
    span: Span,
    body: Expr,
) -> Expr {
    // Fast path: `val x = e`
    if let lumia_syntax::Pattern::Ident(name, _) = pat {
        if ctx.lookup_ctor(name).is_none_or(|c| c.arity != 0) {
            return Expr::Let {
                name: name.clone(),
                value: Box::new(lower_expr(ctx, expr)),
                body: Box::new(body),
                mutable: false,
            };
        }
    }
    if !pattern_irrefutable(ctx, pat) {
        ctx.set_err(
            "val binding pattern must be irrefutable (use `match` for variants / lists / constants)"
                .into(),
            span,
        );
        return body;
    }
    let scrut_name = format!("__valpat_{}", span.start.0);
    let scrut = Expr::Var(scrut_name.clone(), span);
    let (_cond, binds) = pattern_cond(ctx, pat, &scrut, span);
    let mut nested = body;
    for (name, val) in binds.into_iter().rev() {
        nested = Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(nested),
            mutable: false,
        };
    }
    Expr::Let {
        name: scrut_name,
        value: Box::new(lower_expr(ctx, expr)),
        body: Box::new(nested),
        mutable: false,
    }
}

/// `"a${x}b"` → `"a".concat(show(x)).concat("b")` (via builtins).
fn lower_interp(ctx: &LowerCtx, parts: &[lumia_syntax::InterpPart], span: Span) -> Expr {
    let mut pieces: Vec<Expr> = Vec::new();
    for p in parts {
        match p {
            lumia_syntax::InterpPart::Lit(s) => {
                pieces.push(Expr::String(s.clone(), span));
            }
            lumia_syntax::InterpPart::Expr(e) => {
                pieces.push(Expr::BuiltinCall {
                    name: Builtin::Show,
                    args: vec![lower_expr(ctx, e)],
                    span,
                });
            }
        }
    }
    if pieces.is_empty() {
        return Expr::String(String::new(), span);
    }
    let mut acc = pieces.remove(0);
    for p in pieces {
        acc = Expr::BuiltinCall {
            name: Builtin::ListConcat,
            args: vec![acc, p],
            span,
        };
    }
    acc
}

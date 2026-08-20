//! Expression lowering.

mod block;
mod call;
mod interp;
pub(crate) mod product;

use block::lower_block;
use call::{lower_call, lower_call_from_parts};
use interp::lower_interp;
use product::{lower_struct_lit, lower_with};

use super::ctx::LowerCtx;
use super::hof_fuse::{
    try_fuse_hof_all, try_fuse_hof_any, try_fuse_hof_build_filter, try_fuse_hof_build_map,
    try_fuse_hof_contains, try_fuse_hof_drop, try_fuse_hof_find, try_fuse_hof_flat_map,
    try_fuse_hof_fold, try_fuse_hof_get, try_fuse_hof_is_empty, try_fuse_hof_len,
    try_fuse_hof_take, try_fuse_hof_to_list, try_fuse_hof_to_map, try_fuse_hof_to_set,
};
use super::match_arms::{lower_match, lower_match_cond};
use crate::ast::{Builtin, Expr, Fun, Item};
use crate::sym_util::synthetic;
use lumia_syntax::{BinOp, Sym};

pub(crate) fn push_lowered_val(
    ctx: &LowerCtx,
    items: &mut Vec<Item>,
    v: &lumia_syntax::ValItem,
    name: &Sym,
) {
    let body = lower_expr(ctx, &v.body);
    let body = if let Some(params) = &v.params {
        let names: Vec<Sym> = params.iter().map(|(n, _)| n.clone()).collect();
        let param_ann: Vec<Option<String>> = params.iter().map(|(_, t)| t.clone()).collect();
        Expr::Lambda {
            params: names,
            param_ann,
            body: Box::new(body),
            span: v.span,
        }
    } else {
        body
    };
    match body {
        Expr::Lambda {
            params,
            param_ann,
            body,
            span: _,
        } => {
            items.push(Item::Fun(Fun {
                name: name.clone(),
                params,
                param_ann,
                ret_ann: v.ty.clone(),
                body: *body,
                span: v.span,
                is_main: name.as_str() == "main",
                external: None,
                foreign_sig: None,
                foreign_pure: false,
                is_priv: v.is_priv,
            }));
        }
        other => {
            let zero_arg_fun =
                name.as_str() == "main" || matches!(v.body, lumia_syntax::Expr::Block { .. });
            if zero_arg_fun {
                items.push(Item::Fun(Fun {
                    name: name.clone(),
                    params: vec![],
                    param_ann: vec![],
                    ret_ann: v.ty.clone(),
                    body: other,
                    span: v.span,
                    is_main: name.as_str() == "main",
                    external: None,
                    foreign_sig: None,
                    foreign_pure: false,
                    is_priv: v.is_priv,
                }));
            } else {
                items.push(Item::Val {
                    name: name.clone(),
                    body: other,
                    ty: v.ty.clone(),
                    span: v.span,
                    is_priv: v.is_priv,
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
            if let Some(c) = ctx.lookup_ctor(n.as_str()) {
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
        lumia_syntax::Expr::Lambda {
            params,
            param_tys,
            body,
            span,
            ..
        } => Expr::Lambda {
            params: params.clone(),
            param_ann: param_tys.clone(),
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
                    if name == "map" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_build_map(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "filter" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_build_filter(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "flatMap" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_flat_map(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "any" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_any(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "all" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_all(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "find" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_find(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "get" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_get(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "take" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_take(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if (name == "drop" || name == "slice") && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_drop(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "contains" && args.len() == 1 {
                        if let Some(fused) = try_fuse_hof_contains(ctx, left, &args[0], *span) {
                            return fused;
                        }
                    }
                    if name == "toList" && args.is_empty() {
                        if let Some(fused) = try_fuse_hof_to_list(ctx, left, *span) {
                            return fused;
                        }
                    }
                    if name == "toSet" && args.is_empty() {
                        if let Some(fused) = try_fuse_hof_to_set(ctx, left, *span) {
                            return fused;
                        }
                    }
                    if name == "toMap" && args.is_empty() {
                        if let Some(fused) = try_fuse_hof_to_map(ctx, left, *span) {
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
                other => {
                    if let lumia_syntax::Expr::Ident(name, _) = other {
                        if name == "len" {
                            if let Some(fused) = try_fuse_hof_len(ctx, left, *span) {
                                return fused;
                            }
                        }
                        if name == "isEmpty" {
                            if let Some(fused) = try_fuse_hof_is_empty(ctx, left, *span) {
                                return fused;
                            }
                        }
                        if name == "toList" {
                            if let Some(fused) = try_fuse_hof_to_list(ctx, left, *span) {
                                return fused;
                            }
                        }
                        if name == "toSet" {
                            if let Some(fused) = try_fuse_hof_to_set(ctx, left, *span) {
                                return fused;
                            }
                        }
                        if name == "toMap" {
                            if let Some(fused) = try_fuse_hof_to_map(ctx, left, *span) {
                                return fused;
                            }
                        }
                    }
                    lower_call_from_parts(
                        ctx,
                        lower_expr(ctx, other),
                        vec![lower_expr(ctx, left)],
                        *span,
                    )
                }
            }
        }
        lumia_syntax::Expr::Field { base, field, span } => {
            // `xs.len` → len(xs); product fields → adt_field; `p.0` → adt_field;
            // else call field(base)
            if field == "len" {
                if let Some(fused) = try_fuse_hof_len(ctx, base, *span) {
                    return fused;
                }
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
            } else if ctx.is_ambiguous_product_field(field.as_str()) {
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        lower_expr(ctx, base),
                        Expr::Int(-1, *span),
                        Expr::String(field.clone(), *span),
                    ],
                    span: *span,
                }
            } else if let Some((adt_name, idx)) = ctx.lookup_product_field(field.as_str()) {
                // Carry expected product name so ty can reject wrong receivers
                // (global name→index alone is unsound across distinct products).
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        lower_expr(ctx, base),
                        Expr::Int(idx as i64, *span),
                        Expr::String(Sym::from(adt_name), *span),
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
            variant: "".into(),
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
        lumia_syntax::Expr::Spawn { body, span } => lower_spawn(ctx, body, *span),
        lumia_syntax::Expr::Scope {
            scheduler,
            body,
            span,
        } => lower_scope(ctx, scheduler.as_deref(), body, *span),
    }
}

/// `spawn { body }` → `TaskSpawn({ -> body })`.
/// Zero-param thunk: no-capture → FunRef `fn()→T`; captures → `fn(env)→T` (closure ABI).
fn lower_spawn(ctx: &LowerCtx, body: &lumia_syntax::Expr, span: lumia_syntax::Span) -> Expr {
    let lowered = lower_expr(ctx, body);
    let thunk = match lowered {
        Expr::Lambda {
            ref params,
            body: _,
            ..
        } if params.is_empty() => lowered,
        other => Expr::Lambda {
            params: vec![],
            param_ann: vec![],
            body: Box::new(other),
            span,
        },
    };
    Expr::BuiltinCall {
        name: Builtin::TaskSpawn,
        args: vec![thunk],
        span,
    }
}

/// `scope { body }` / `scope(sched) { body }` →
/// `ScopeEnter(sched_or_0); let __r = body'; ScopeLeave(); __r`.
///
/// Every `return` in `body` (not inside nested lambdas) is rewritten to
/// `let r = …; ScopeLeave(); return r` so early exit still pops the scope
/// (nested scopes → LIFO: inner leave runs before outer). The return value is
/// evaluated *before* leave so children are not joined while the value still
/// needs them (e.g. `return ch.recv()` with a producer sibling).
fn lower_scope(
    ctx: &LowerCtx,
    scheduler: Option<&lumia_syntax::Expr>,
    body: &lumia_syntax::Expr,
    span: lumia_syntax::Span,
) -> Expr {
    let sched = match scheduler {
        Some(e) => lower_expr(ctx, e),
        None => Expr::Int(0, span),
    };
    let body = prepend_scope_leave_on_return(lower_expr(ctx, body), span);
    let result = synthetic(format!("__scope_r_{}", span.start.0));
    Expr::Seq {
        stmts: vec![
            Expr::BuiltinCall {
                name: Builtin::ScopeEnter,
                args: vec![sched],
                span,
            },
            Expr::Let {
                name: result.clone(),
                value: Box::new(body),
                body: Box::new(Expr::Seq {
                    stmts: vec![
                        Expr::BuiltinCall {
                            name: Builtin::ScopeLeave,
                            args: vec![],
                            span,
                        },
                        Expr::Var(result, span),
                    ],
                    span,
                }),
                mutable: false,
                ty: None,
            },
        ],
        span,
    }
}

/// Replace `return v` with `let r = v; ScopeLeave(); return r`. Does not enter
/// lambdas (those returns belong to the closure / spawn thunk, not this scope).
fn prepend_scope_leave_on_return(expr: Expr, scope_span: lumia_syntax::Span) -> Expr {
    let leave = Expr::BuiltinCall {
        name: Builtin::ScopeLeave,
        args: vec![],
        span: scope_span,
    };
    match expr {
        Expr::Return { value, span } => {
            let tmp = synthetic(format!("__scope_ret_{}", span.start.0));
            Expr::Let {
                name: tmp.clone(),
                value: Box::new(prepend_scope_leave_on_return(*value, scope_span)),
                body: Box::new(Expr::Seq {
                    stmts: vec![
                        leave,
                        Expr::Return {
                            value: Box::new(Expr::Var(tmp, span)),
                            span,
                        },
                    ],
                    span,
                }),
                mutable: false,
                ty: None,
            }
        }
        // Returns inside closures/spawn thunks exit that function, not this scope.
        Expr::Lambda {
            params,
            param_ann,
            body,
            span,
        } => Expr::Lambda {
            params,
            param_ann,
            body,
            span,
        },
        Expr::Let {
            name,
            value,
            body,
            mutable,
            ty,
        } => Expr::Let {
            name,
            value: Box::new(prepend_scope_leave_on_return(*value, scope_span)),
            body: Box::new(prepend_scope_leave_on_return(*body, scope_span)),
            mutable,
            ty,
        },
        Expr::Assign { name, value, span } => Expr::Assign {
            name,
            value: Box::new(prepend_scope_leave_on_return(*value, scope_span)),
            span,
        },
        Expr::Call { callee, args, span } => Expr::Call {
            callee: Box::new(prepend_scope_leave_on_return(*callee, scope_span)),
            args: args
                .into_iter()
                .map(|a| prepend_scope_leave_on_return(a, scope_span))
                .collect(),
            span,
        },
        Expr::Binary {
            op,
            left,
            right,
            span,
        } => Expr::Binary {
            op,
            left: Box::new(prepend_scope_leave_on_return(*left, scope_span)),
            right: Box::new(prepend_scope_leave_on_return(*right, scope_span)),
            span,
        },
        Expr::Unary { op, expr, span } => Expr::Unary {
            op,
            expr: Box::new(prepend_scope_leave_on_return(*expr, scope_span)),
            span,
        },
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span,
        } => Expr::If {
            cond: Box::new(prepend_scope_leave_on_return(*cond, scope_span)),
            then_branch: Box::new(prepend_scope_leave_on_return(*then_branch, scope_span)),
            else_branch: Box::new(prepend_scope_leave_on_return(*else_branch, scope_span)),
            span,
        },
        Expr::Loop {
            cond,
            body,
            step,
            span,
        } => Expr::Loop {
            cond: Box::new(prepend_scope_leave_on_return(*cond, scope_span)),
            body: Box::new(prepend_scope_leave_on_return(*body, scope_span)),
            step: step.map(|s| Box::new(prepend_scope_leave_on_return(*s, scope_span))),
            span,
        },
        Expr::Alt {
            scrutinee,
            alt,
            span,
        } => Expr::Alt {
            scrutinee: Box::new(prepend_scope_leave_on_return(*scrutinee, scope_span)),
            alt: Box::new(prepend_scope_leave_on_return(*alt, scope_span)),
            span,
        },
        Expr::With { base, fields, span } => Expr::With {
            base: Box::new(prepend_scope_leave_on_return(*base, scope_span)),
            fields: fields
                .into_iter()
                .map(|(n, e)| (n, prepend_scope_leave_on_return(e, scope_span)))
                .collect(),
            span,
        },
        Expr::Seq { stmts, span } => Expr::Seq {
            stmts: stmts
                .into_iter()
                .map(|s| prepend_scope_leave_on_return(s, scope_span))
                .collect(),
            span,
        },
        Expr::BuiltinCall { name, args, span } => Expr::BuiltinCall {
            name,
            args: args
                .into_iter()
                .map(|a| prepend_scope_leave_on_return(a, scope_span))
                .collect(),
            span,
        },
        Expr::AdtNew {
            adt_name,
            variant,
            tag,
            args,
            span,
        } => Expr::AdtNew {
            adt_name,
            variant,
            tag,
            args: args
                .into_iter()
                .map(|a| prepend_scope_leave_on_return(a, scope_span))
                .collect(),
            span,
        },
        other => other,
    }
}

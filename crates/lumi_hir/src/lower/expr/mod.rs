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
use super::hof_fuse::try_fuse_hof_fold;
use super::match_arms::{lower_match, lower_match_cond};
use crate::ast::{Builtin, Expr, Fun, Item};
use lumi_syntax::BinOp;

pub(crate) fn push_lowered_val(
    ctx: &LowerCtx,
    items: &mut Vec<Item>,
    v: &lumi_syntax::ValItem,
    name: &str,
) {
    let body = lower_expr(ctx, &v.body);
    let body = if let Some(params) = &v.params {
        let names: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
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
    // DESIGN §4.4: peel a surface lambda (`val f = { x -> … }` / `{ -> … }`) or
    // a `val f(params) = …` wrapper into Item::Fun. Do **not** peel a Lambda that
    // is merely the *value* of a block (`val f = { { -> body } }` must stay a
    // zero-arg Fun that *returns* a thunk).
    let peel_lambda = matches!(v.body, lumi_syntax::Expr::Lambda { .. }) || v.params.is_some();
    match body {
        Expr::Lambda {
            params,
            param_ann,
            body,
            span: _,
        } if peel_lambda => {
            items.push(Item::Fun(Fun {
                name: name.to_string(),
                params,
                param_ann,
                ret_ann: v.ty.clone(),
                body: *body,
                is_main: name == "main",
                external: None,
                foreign_sig: None,
                foreign_pure: false,
            }));
        }
        other => {
            let zero_arg_fun = name == "main" || matches!(v.body, lumi_syntax::Expr::Block { .. });
            if zero_arg_fun {
                items.push(Item::Fun(Fun {
                    name: name.to_string(),
                    params: vec![],
                    param_ann: vec![],
                    ret_ann: v.ty.clone(),
                    body: other,
                    is_main: name == "main",
                    external: None,
                    foreign_sig: None,
                    foreign_pure: false,
                }));
            } else {
                items.push(Item::Val {
                    name: name.to_string(),
                    body: other,
                    ty: v.ty.clone(),
                });
            }
        }
    }
}
pub(crate) fn lower_expr(ctx: &LowerCtx, e: &lumi_syntax::Expr) -> Expr {
    match e {
        lumi_syntax::Expr::Int(n, s) => Expr::Int(*n, *s),
        lumi_syntax::Expr::Float(n, s) => Expr::Float(*n, *s),
        lumi_syntax::Expr::Bool(b, s) => Expr::Bool(*b, *s),
        lumi_syntax::Expr::String(t, s) => Expr::String(t.clone(), *s),
        lumi_syntax::Expr::Interp { parts, span } => lower_interp(ctx, parts, *span),
        lumi_syntax::Expr::Char(c, s) => Expr::Char(*c, *s),
        lumi_syntax::Expr::Ident(n, s) => {
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
        lumi_syntax::Expr::Block { stmts, tail, span } => {
            lower_block(ctx, stmts, tail.as_deref(), *span)
        }
        lumi_syntax::Expr::Lambda {
            params,
            param_tys,
            body,
            span,
        } => Expr::Lambda {
            params: params.clone(),
            param_ann: param_tys.clone(),
            body: Box::new(lower_expr(ctx, body)),
            span: *span,
        },
        lumi_syntax::Expr::Call { callee, args, span } => lower_call(ctx, callee, args, *span),
        lumi_syntax::Expr::Binary {
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
        lumi_syntax::Expr::Unary { op, expr, span } => Expr::Unary {
            op: *op,
            expr: Box::new(lower_expr(ctx, expr)),
            span: *span,
        },
        lumi_syntax::Expr::If {
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
        lumi_syntax::Expr::Pipeline { left, right, span } => {
            // Fuse `xs >> map … >> filter … >> fold(z, g)` before expanding intermediates.
            if let lumi_syntax::Expr::Call { callee, args, .. } = right.as_ref() {
                if let lumi_syntax::Expr::Ident(name, _) = callee.as_ref() {
                    if name == "fold" && args.len() == 2 {
                        if let Some(fused) = try_fuse_hof_fold(ctx, left, &args[0], &args[1], *span)
                        {
                            return fused;
                        }
                    }
                }
            }
            match right.as_ref() {
                lumi_syntax::Expr::Call { callee, args, .. } => {
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
        lumi_syntax::Expr::Field { base, field, span } => {
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
                // Defer index resolution until ty knows the receiver product.
                Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        lower_expr(ctx, base),
                        Expr::Int(-1, *span),
                        Expr::String(field.clone(), *span),
                    ],
                    span: *span,
                }
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
        lumi_syntax::Expr::StructLit { name, fields, span } => {
            lower_struct_lit(ctx, name, fields, *span)
        }
        lumi_syntax::Expr::With { base, fields, span } => lower_with(ctx, base, fields, *span),
        lumi_syntax::Expr::TupleLit { elems, span } => Expr::AdtNew {
            adt_name: "__Tuple".into(),
            variant: String::new(),
            tag: 0,
            args: elems.iter().map(|e| lower_expr(ctx, e)).collect(),
            span: *span,
        },
        lumi_syntax::Expr::ListLit { elems, span } => Expr::Call {
            callee: Box::new(Expr::Var("listOf".into(), *span)),
            args: elems.iter().map(|e| lower_expr(ctx, e)).collect(),
            span: *span,
        },
        lumi_syntax::Expr::Match {
            scrutinee,
            arms,
            span,
        } => lower_match(ctx, scrutinee, arms, *span),
        lumi_syntax::Expr::MatchCond { arms, span } => lower_match_cond(ctx, arms, *span),
        lumi_syntax::Expr::Return { value, span } => Expr::Return {
            value: Box::new(lower_expr(ctx, value)),
            span: *span,
        },
        lumi_syntax::Expr::Alt {
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

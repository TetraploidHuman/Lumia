//! List filter / flatMap desugaring.

use super::{
    append_assign, concat_assign, list_accum, resolve_unary_callback, with_fun_bind, UnaryCallback,
};
use crate::ast::Expr;
use crate::lower::{empty_list, LowerCtx};
use lumia_syntax::Span;

pub(crate) fn lower_list_filter(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    match resolve_unary_callback(f, span, "flt") {
        UnaryCallback::Inline { param, body } => {
            lower_list_filter_inline(ctx, list, param, body, span)
        }
        UnaryCallback::Bound { f, f_name, x } => {
            lower_list_filter_call(ctx, list, f, f_name, x, span)
        }
    }
}

fn lower_list_filter_inline(ctx: &LowerCtx, list: Expr, x: String, body: Expr, span: Span) -> Expr {
    let acc = format!("__flt_acc_{}", span.start.0);
    let append = append_assign(&acc, Expr::Var(x.clone(), span), span);
    let step = Expr::If {
        cond: Box::new(body),
        then_branch: Box::new(append),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    list_accum(ctx, acc, empty_list(span), &x, list, step, span)
}

fn lower_list_filter_call(
    ctx: &LowerCtx,
    list: Expr,
    f: Expr,
    f_name: String,
    x: String,
    span: Span,
) -> Expr {
    let acc = format!("__flt_acc_{}", span.start.0);
    let pred = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(x.clone(), span)],
        span,
    };
    let append = append_assign(&acc, Expr::Var(x.clone(), span), span);
    let step = Expr::If {
        cond: Box::new(pred),
        then_branch: Box::new(append),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    with_fun_bind(
        Some((f_name, f)),
        list_accum(ctx, acc, empty_list(span), &x, list, step, span),
    )
}

pub(crate) fn apply_pred(f: &Expr, x: Expr, span: Span) -> Expr {
    match f {
        Expr::Lambda { params, body, .. } if params.len() == 1 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(x),
            body: body.clone(),
            mutable: false,
        },
        _ => Expr::Call {
            callee: Box::new(f.clone()),
            args: vec![x],
            span,
        },
    }
}

/// `xs.flatMap(f)` where `f: T -> List[U]` → concat mapped lists.
pub(crate) fn lower_list_flat_map(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    let acc = format!("__fmap_acc_{}", span.start.0);
    match resolve_unary_callback(f, span, "fmap") {
        UnaryCallback::Inline { param, body } => {
            let x = format!("__fmap_x_{}", span.start.0);
            let mapped = Expr::Let {
                name: param,
                value: Box::new(Expr::Var(x.clone(), span)),
                body: Box::new(body),
                mutable: false,
            };
            let step = concat_assign(&acc, mapped, span);
            list_accum(ctx, acc, empty_list(span), &x, list, step, span)
        }
        UnaryCallback::Bound { f, f_name, x } => {
            let mapped = Expr::Call {
                callee: Box::new(Expr::Var(f_name.clone(), span)),
                args: vec![Expr::Var(x.clone(), span)],
                span,
            };
            let step = concat_assign(&acc, mapped, span);
            with_fun_bind(
                Some((f_name, f)),
                list_accum(ctx, acc, empty_list(span), &x, list, step, span),
            )
        }
    }
}

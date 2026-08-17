//! List filter / flatMap desugaring.

use super::{
    append_assign, concat_assign, list_accum, resolve_unary_callback, with_fun_bind, UnaryCallback,
};
use crate::ast::Expr;
use crate::lower::{empty_list, LowerCtx};
use lumia_syntax::Span;

pub(crate) fn lower_list_filter(_ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    match resolve_unary_callback(f, span, "flt") {
        UnaryCallback::Inline {
            param,
            param_ty,
            body,
        } => lower_list_filter_inline(_ctx, list, param, param_ty, body, span),
        UnaryCallback::Bound { f, f_name, x } => {
            lower_list_filter_call(_ctx, list, f, f_name, x, span)
        }
    }
}

fn lower_list_filter_inline(
    _ctx: &LowerCtx,
    list: Expr,
    param: String,
    param_ty: Option<String>,
    body: Expr,
    span: Span,
) -> Expr {
    let acc = format!("{}_{}", crate::desugar_slots::FLT_ACC_PREFIX, span.start.0);
    let x = format!("__flt_x_{}", span.start.0);
    let pred = Expr::Let {
        name: param,
        value: Box::new(Expr::Var(x.clone(), span)),
        body: Box::new(body),
        mutable: false,
        ty: param_ty,
    };
    let append = append_assign(&acc, Expr::Var(x.clone(), span), span);
    let step = Expr::If {
        cond: Box::new(pred),
        then_branch: Box::new(append),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    list_accum(acc, empty_list(span), &x, list, step, span)
}

fn lower_list_filter_call(
    _ctx: &LowerCtx,
    list: Expr,
    f: Expr,
    f_name: String,
    x: String,
    span: Span,
) -> Expr {
    let acc = format!("{}_{}", crate::desugar_slots::FLT_ACC_PREFIX, span.start.0);
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
        list_accum(acc, empty_list(span), &x, list, step, span),
    )
}

pub(crate) fn apply_pred(f: &Expr, x: Expr, span: Span) -> Expr {
    match f {
        Expr::Lambda {
            params,
            param_ann,
            body,
            ..
        } if params.len() == 1 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(x),
            body: body.clone(),
            mutable: false,
            ty: param_ann.first().cloned().flatten(),
        },
        _ => Expr::Call {
            callee: Box::new(f.clone()),
            args: vec![x],
            span,
        },
    }
}

/// `xs.flatMap(f)` where `f: T -> List[U]` → concat mapped lists.
pub(crate) fn lower_list_flat_map(_ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    let acc = format!("{}_{}", crate::desugar_slots::FMAP_ACC_PREFIX, span.start.0);
    match resolve_unary_callback(f, span, "fmap") {
        UnaryCallback::Inline {
            param,
            param_ty,
            body,
        } => {
            let x = format!("__fmap_x_{}", span.start.0);
            let mapped = Expr::Let {
                name: param,
                value: Box::new(Expr::Var(x.clone(), span)),
                body: Box::new(body),
                mutable: false,
                ty: param_ty,
            };
            let step = concat_assign(&acc, mapped, span);
            list_accum(acc, empty_list(span), &x, list, step, span)
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
                list_accum(acc, empty_list(span), &x, list, step, span),
            )
        }
    }
}

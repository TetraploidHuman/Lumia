//! List higher-order function desugaring (map/filter/fold/any/all/find).

mod filter;
mod fold;
mod for_each;
mod map;
mod search;

use crate::ast::Expr;
use crate::lower::{counter_for_in, for_each_elem, LowerCtx};
use lumi_syntax::Span;

pub(crate) use filter::{lower_list_filter, lower_list_flat_map};
pub use fold::desugar_list_fold_sequential;
pub(crate) use fold::{lower_list_fold, syntax_fold_body_is_associative};
pub(crate) use for_each::lower_list_for_each;
pub use map::desugar_list_map_sequential;
pub(crate) use map::{lower_list_map, lower_list_sort_by};
pub(crate) use search::{lower_list_all, lower_list_any, lower_list_find};

pub(crate) fn list_accum(
    ctx: &LowerCtx,
    acc: String,
    init: Expr,
    x: &str,
    list: Expr,
    step: Expr,
    span: Span,
) -> Expr {
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(ctx, x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// Shared accumulate-over-range skeleton: `let mut acc = init; for x in start..end { step }; acc`.
pub(crate) fn range_accum(
    ctx: &LowerCtx,
    acc: String,
    init: Expr,
    x: &str,
    start: Expr,
    end: Expr,
    inclusive: bool,
    step: Expr,
    span: Span,
) -> Expr {
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                counter_for_in(ctx, x, start, end, inclusive, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// Optional `let f = … in body` when the callback is not a lambda.
pub(crate) fn with_fun_bind(f_bind: Option<(String, Expr)>, body: Expr) -> Expr {
    match f_bind {
        Some((name, val)) => Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(body),
            mutable: false,
            ty: None,
        },
        None => body,
    }
}

/// Unary callback: inline lambda body vs bound function call.
pub(crate) enum UnaryCallback {
    Inline {
        param: String,
        param_ty: Option<String>,
        body: Expr,
    },
    Bound {
        f: Expr,
        f_name: String,
        x: String,
    },
}

pub(crate) fn resolve_unary_callback(f: Expr, span: Span, prefix: &str) -> UnaryCallback {
    match f {
        Expr::Lambda {
            params,
            param_ann,
            body,
            ..
        } if params.len() == 1 => UnaryCallback::Inline {
            param: params[0].clone(),
            param_ty: param_ann.first().cloned().flatten(),
            body: *body,
        },
        f => {
            let f_name = format!("__{prefix}_f_{}", span.start.0);
            let x = format!("__{prefix}_x_{}", span.start.0);
            UnaryCallback::Bound { f, f_name, x }
        }
    }
}

/// Binary callback: inline lambda body vs bound function call.
pub(crate) enum BinaryCallback {
    Inline {
        acc: String,
        acc_ty: Option<String>,
        x: String,
        x_ty: Option<String>,
        body: Expr,
    },
    Bound {
        f: Expr,
        f_name: String,
        acc: String,
        x: String,
    },
}

pub(crate) fn resolve_binary_callback(f: Expr, span: Span, prefix: &str) -> BinaryCallback {
    match f {
        Expr::Lambda {
            params,
            param_ann,
            body,
            ..
        } if params.len() == 2 => BinaryCallback::Inline {
            acc: params[0].clone(),
            acc_ty: param_ann.first().cloned().flatten(),
            x: params[1].clone(),
            x_ty: param_ann.get(1).cloned().flatten(),
            body: *body,
        },
        f => {
            let f_name = format!("__{prefix}_f_{}", span.start.0);
            let x = format!("__{prefix}_x_{}", span.start.0);
            let acc = format!("__{prefix}_acc_{}", span.start.0);
            BinaryCallback::Bound { f, f_name, acc, x }
        }
    }
}

pub(crate) fn option_some(ctx: &LowerCtx, x: Expr, span: Span) -> Expr {
    match ctx.lookup_ctor("Some") {
        Some(c) => Expr::AdtNew {
            adt_name: c.adt_name,
            variant: "Some".into(),
            tag: c.tag,
            args: vec![x],
            span,
        },
        None => Expr::Call {
            callee: Box::new(Expr::Var("Some".into(), span)),
            args: vec![x],
            span,
        },
    }
}

pub(crate) fn option_none(ctx: &LowerCtx, span: Span) -> Expr {
    match ctx.lookup_ctor("None") {
        Some(c) => Expr::AdtNew {
            adt_name: c.adt_name,
            variant: "None".into(),
            tag: c.tag,
            args: vec![],
            span,
        },
        None => Expr::Call {
            callee: Box::new(Expr::Var("None".into(), span)),
            args: vec![],
            span,
        },
    }
}

/// Bind non-lambda `f` to a temp; lambdas stay inline.
pub(crate) fn bind_fun(f: Expr, span: Span) -> (Option<(String, Expr)>, Expr) {
    match &f {
        Expr::Lambda { .. } => (None, f),
        _ => {
            let name = format!("__pred_f_{}", span.start.0);
            (Some((name.clone(), f)), Expr::Var(name, span))
        }
    }
}

/// `acc = ListAppend(acc, elem)` assign used by map/filter desugars.
pub(crate) fn append_assign(acc: &str, elem: Expr, span: Span) -> Expr {
    use crate::ast::Builtin;
    Expr::Assign {
        name: acc.to_string(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.to_string(), span), elem],
            span,
        }),
        span,
    }
}

/// `acc = ListConcat(acc, chunk)` assign used by flatMap.
pub(crate) fn concat_assign(acc: &str, chunk: Expr, span: Span) -> Expr {
    use crate::ast::Builtin;
    Expr::Assign {
        name: acc.to_string(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListConcat,
            args: vec![Expr::Var(acc.to_string(), span), chunk],
            span,
        }),
        span,
    }
}

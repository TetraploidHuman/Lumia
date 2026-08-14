//! Collection conversion and set operations.

use super::ctx::LowerCtx;
use super::for_loops::{empty_list, empty_map, empty_set, for_each_elem, list_for_in};
use crate::ast::{Builtin, Expr};
use lumia_syntax::Span;

pub(crate) fn lower_to_set(ctx: &LowerCtx, list: Expr, span: Span) -> Expr {
    let acc = format!("__toset_acc_{}", span.start.0);
    let x = format!("__toset_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::SetInsert,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_set(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(ctx, &x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

pub(crate) fn lower_to_list(ctx: &LowerCtx, col: Expr, span: Span) -> Expr {
    let acc = format!("__tolist_acc_{}", span.start.0);
    let x = format!("__tolist_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_list(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(ctx, &x, col, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

/// `pairs.toMap()` — each element is a 2-tuple `(k, v)`.
pub(crate) fn lower_to_map(ctx: &LowerCtx, pairs: Expr, span: Span) -> Expr {
    let acc = format!("__tomap_acc_{}", span.start.0);
    let p = format!("__tomap_p_{}", span.start.0);
    let k = Expr::BuiltinCall {
        name: Builtin::AdtField,
        args: vec![Expr::Var(p.clone(), span), Expr::Int(0, span)],
        span,
    };
    let v = Expr::BuiltinCall {
        name: Builtin::AdtField,
        args: vec![Expr::Var(p.clone(), span), Expr::Int(1, span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::MapSet,
            args: vec![Expr::Var(acc.clone(), span), k, v],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(empty_map(span)),
        body: Box::new(Expr::Seq {
            stmts: vec![
                list_for_in(ctx, &p, pairs, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

pub(crate) fn lower_set_union(ctx: &LowerCtx, a: Expr, b: Expr, span: Span) -> Expr {
    let acc = format!("__union_acc_{}", span.start.0);
    let x = format!("__union_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::SetInsert,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(a),
        body: Box::new(Expr::Seq {
            stmts: vec![list_for_in(ctx, &x, b, step, span), Expr::Var(acc, span)],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

pub(crate) fn lower_set_intersect(ctx: &LowerCtx, a: Expr, b: Expr, span: Span) -> Expr {
    let acc = format!("__isect_acc_{}", span.start.0);
    let other = format!("__isect_b_{}", span.start.0);
    let x = format!("__isect_x_{}", span.start.0);
    let insert = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::SetInsert,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    let step = Expr::If {
        cond: Box::new(Expr::BuiltinCall {
            name: Builtin::Contains,
            args: vec![Expr::Var(other.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        then_branch: Box::new(insert),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    Expr::Let {
        name: other,
        value: Box::new(b),
        body: Box::new(Expr::Let {
            name: acc.clone(),
            value: Box::new(empty_set(span)),
            body: Box::new(Expr::Seq {
                stmts: vec![list_for_in(ctx, &x, a, step, span), Expr::Var(acc, span)],
                span,
            }),
            mutable: true,
            ty: None,
        }),
        mutable: false,
        ty: None,
    }
}

pub(crate) fn lower_set_diff(ctx: &LowerCtx, a: Expr, b: Expr, span: Span) -> Expr {
    let acc = format!("__diff_acc_{}", span.start.0);
    let x = format!("__diff_x_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::MapRemove,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    Expr::Let {
        name: acc.clone(),
        value: Box::new(a),
        body: Box::new(Expr::Seq {
            stmts: vec![list_for_in(ctx, &x, b, step, span), Expr::Var(acc, span)],
            span,
        }),
        mutable: true,
        ty: None,
    }
}

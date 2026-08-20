//! List any / all / find desugaring.

use super::filter::apply_pred;
use super::{bind_fun, list_accum, option_none, option_some, with_fun_bind};
use crate::ast::Expr;
use crate::lower::LowerCtx;
use crate::sym_util::synthetic;
use lumia_syntax::Span;

enum ListSearchKind {
    Any,
    All,
    Find,
}

/// Shared short-circuit search: `any` / `all` / `find`.
fn list_search(ctx: &LowerCtx, list: Expr, f: Expr, span: Span, kind: ListSearchKind) -> Expr {
    let (prefix, init) = match kind {
        ListSearchKind::Any => ("any", Expr::Bool(false, span)),
        ListSearchKind::All => ("all", Expr::Bool(true, span)),
        ListSearchKind::Find => ("find", option_none(ctx, span)),
    };
    let acc = synthetic(format!("__{prefix}_acc_{}", span.start.0));
    let x = synthetic(format!("__{prefix}_x_{}", span.start.0));
    let (f_bind, pred_f) = bind_fun(f, span);
    let pred = apply_pred(&pred_f, Expr::Var(x.clone(), span), span);
    let step = match kind {
        ListSearchKind::Any => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(Expr::Bool(true, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
        ListSearchKind::All => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Unit(span)),
            else_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(Expr::Bool(false, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            span,
        },
        ListSearchKind::Find => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(option_some(ctx, Expr::Var(x.clone(), span), span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
    };
    with_fun_bind(f_bind, list_accum(acc, init, x.as_str(), list, step, span))
}

pub(crate) fn lower_list_any(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    list_search(ctx, list, f, span, ListSearchKind::Any)
}

pub(crate) fn lower_list_all(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    list_search(ctx, list, f, span, ListSearchKind::All)
}

pub(crate) fn lower_list_find(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    list_search(ctx, list, f, span, ListSearchKind::Find)
}

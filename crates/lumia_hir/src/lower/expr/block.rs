//! Block / val-pattern lowering.

use super::super::ctx::LowerCtx;
use super::super::for_loops::lower_for_in;
use super::super::hof_fuse::{try_deforest_hof_let, try_fuse_hof_for_in};
use super::lower_expr;
use crate::ast::Expr;
use crate::match_check::{pattern_cond, pattern_irrefutable};
use crate::sym_util::synthetic;
use lumia_syntax::Span;

pub(super) fn lower_block(
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
            lumia_syntax::Stmt::Val {
                pat,
                ty,
                expr,
                span: s,
            } => lower_val_pat(
                ctx,
                pat,
                ty.as_deref(),
                expr,
                *s,
                fold(ctx, rest, tail, span),
            ),
            lumia_syntax::Stmt::Var {
                name,
                ty,
                expr,
                span: s,
            } => {
                let _ = s;
                Expr::Let {
                    name: name.clone(),
                    value: Box::new(lower_expr(ctx, expr)),
                    body: Box::new(fold(ctx, rest, tail, span)),
                    mutable: true,
                    ty: ty.clone(),
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
                let loop_e = match binding {
                    lumia_syntax::ForBinding::Name(name) => {
                        try_fuse_hof_for_in(ctx, name, iter, body, *s)
                            .unwrap_or_else(|| lower_for_in(ctx, binding, iter, body, *s))
                    }
                    lumia_syntax::ForBinding::Pair(_, _) => {
                        lower_for_in(ctx, binding, iter, body, *s)
                    }
                };
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
pub(super) fn lower_val_pat(
    ctx: &LowerCtx,
    pat: &lumia_syntax::Pattern,
    ty: Option<&str>,
    expr: &lumia_syntax::Expr,
    span: Span,
    body: Expr,
) -> Expr {
    // Fast path: `val x = e` / `val x: T = e`
    if let lumia_syntax::Pattern::Ident(name, _) = pat {
        if ctx.lookup_ctor(name.as_str()).is_none_or(|c| c.arity != 0) {
            // DESIGN §7.3: `val ys = pipe.map/filter…` used only via get/len → deforest.
            if let Some(deforested) = try_deforest_hof_let(ctx, name.as_str(), expr, &body, span) {
                return deforested;
            }
            return Expr::Let {
                name: name.clone(),
                value: Box::new(lower_expr(ctx, expr)),
                body: Box::new(body),
                mutable: false,
                ty: ty.map(|s| s.to_string()),
            };
        }
    }
    if ty.is_some() {
        ctx.set_err(
            "type ascription is only allowed on simple `val` binders".into(),
            span,
        );
    }
    if !pattern_irrefutable(ctx, pat) {
        ctx.set_err(
            "val binding pattern must be irrefutable (use `match` for variants / lists / constants)"
                .into(),
            span,
        );
        return body;
    }
    let scrut_name = synthetic(format!("__valpat_{}", span.start.0));
    let scrut = Expr::Var(scrut_name.clone(), span);
    let (_cond, binds) = pattern_cond(ctx, pat, &scrut, span);
    let mut nested = body;
    for (name, val) in binds.into_iter().rev() {
        nested = Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(nested),
            mutable: false,
            ty: None,
        };
    }
    Expr::Let {
        name: scrut_name,
        value: Box::new(lower_expr(ctx, expr)),
        body: Box::new(nested),
        mutable: false,
        ty: None,
    }
}

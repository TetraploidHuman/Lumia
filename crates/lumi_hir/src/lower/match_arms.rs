//! Match expression lowering.

use super::ctx::LowerCtx;
use super::expr::lower_expr;
use crate::ast::{Builtin, Expr};
use crate::match_check::{pattern_cond, pattern_irrefutable, short_and};
use lumi_syntax::{Pattern, Span};

pub(crate) fn lower_match_cond(
    ctx: &LowerCtx,
    arms: &[lumi_syntax::MatchCondArm],
    span: Span,
) -> Expr {
    fold_match_cond_arms(ctx, arms, span)
}

fn fold_match_cond_arms(ctx: &LowerCtx, arms: &[lumi_syntax::MatchCondArm], span: Span) -> Expr {
    if arms.is_empty() {
        return Expr::Unit(span);
    }
    let (arm, rest) = arms.split_first().unwrap();
    match &arm.cond {
        None => lower_expr(ctx, &arm.body),
        Some(cond) => {
            let else_body = if rest.is_empty() {
                Expr::Unit(span)
            } else {
                fold_match_cond_arms(ctx, rest, span)
            };
            Expr::If {
                cond: Box::new(lower_expr(ctx, cond)),
                then_branch: Box::new(lower_expr(ctx, &arm.body)),
                else_branch: Box::new(else_body),
                span,
            }
        }
    }
}
pub(crate) fn lower_match(
    ctx: &LowerCtx,
    scrutinee: &lumi_syntax::Expr,
    arms: &[lumi_syntax::MatchArm],
    span: Span,
) -> Expr {
    let scrut = "__match_s".to_string();
    let expanded = expand_or_arms(arms);
    let body = fold_match_arms(ctx, &expanded, &scrut, span);
    Expr::Let {
        name: scrut,
        value: Box::new(lower_expr(ctx, scrutinee)),
        body: Box::new(body),
        mutable: false,
        ty: None,
    }
}

/// Top-level `A | B -> body` → two arms (correct bindings per alternative).
fn expand_or_arms(arms: &[lumi_syntax::MatchArm]) -> Vec<lumi_syntax::MatchArm> {
    let mut out = Vec::new();
    for arm in arms {
        match &arm.pattern {
            Pattern::Or(pats, _) if pats.len() > 1 => {
                for p in pats {
                    out.push(lumi_syntax::MatchArm {
                        pattern: p.clone(),
                        guard: arm.guard.clone(),
                        body: arm.body.clone(),
                        span: arm.span,
                    });
                }
            }
            _ => out.push(arm.clone()),
        }
    }
    out
}

fn fold_match_arms(
    ctx: &LowerCtx,
    arms: &[lumi_syntax::MatchArm],
    scrut: &str,
    span: Span,
) -> Expr {
    if arms.is_empty() {
        return Expr::BuiltinCall {
            name: Builtin::MatchFail,
            args: vec![],
            span,
        };
    }
    let scrut_e = Expr::Var(scrut.into(), span);
    let (arm, rest) = arms.split_first().unwrap();
    let (pat_cond, binds) = pattern_cond(ctx, &arm.pattern, &scrut_e, span);
    let cond = if let Some(g) = &arm.guard {
        // Pattern bindings must be in scope for the guard (`x if x > 0`).
        let mut guard_e = lower_expr(ctx, g);
        for (name, val) in binds.iter().rev() {
            guard_e = Expr::Let {
                name: name.clone(),
                value: Box::new(val.clone()),
                body: Box::new(guard_e),
                mutable: false,
                ty: None,
            };
        }
        // Short-circuit: do not evaluate guard (or its field loads) if pat fails.
        short_and(pat_cond, guard_e, span)
    } else {
        pat_cond
    };
    let mut then_body = lower_expr(ctx, &arm.body);
    for (name, val) in binds.into_iter().rev() {
        then_body = Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(then_body),
            mutable: false,
            ty: None,
        };
    }
    // Always test the pattern — including the last arm (unless irrefutable).
    if rest.is_empty() {
        if pattern_irrefutable(ctx, &arm.pattern) {
            return then_body;
        }
        return Expr::If {
            cond: Box::new(cond),
            then_branch: Box::new(then_body),
            else_branch: Box::new(Expr::BuiltinCall {
                name: Builtin::MatchFail,
                args: vec![],
                span,
            }),
            span,
        };
    }
    let else_body = fold_match_arms(ctx, rest, scrut, span);
    Expr::If {
        cond: Box::new(cond),
        then_branch: Box::new(then_body),
        else_branch: Box::new(else_body),
        span,
    }
}

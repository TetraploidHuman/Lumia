//! Auto-parallel eligibility finalization.

use crate::types::{expr_span, Type, TypedModule};
use lumi_hir::{
    desugar_list_fold_sequential, desugar_list_map_sequential, Builtin, Expr, Item, LowerCtx,
};

pub(crate) fn is_par_scalar(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Bool | Type::Float)
}

pub(crate) fn type_at_span(
    type_at: &[(lumi_syntax::Span, Type)],
    span: lumi_syntax::Span,
) -> Option<Type> {
    type_at
        .iter()
        .rev()
        .find(|(s, _)| *s == span)
        .map(|(_, t)| t.clone())
}

pub(crate) fn list_par_map_eligible(list_ty: &Type, fun_ty: &Type) -> bool {
    let elem = match list_ty {
        Type::List(e) => e.as_ref(),
        _ => return false,
    };
    if !is_par_scalar(elem) {
        return false;
    }
    match fun_ty {
        Type::Fun(params, out, eff) => {
            if eff.has_io() || !is_par_scalar(out) {
                return false;
            }
            params.first().is_none_or(is_par_scalar)
        }
        _ => false,
    }
}

pub(crate) fn list_par_fold_eligible(list_ty: &Type, init_ty: &Type, fun_ty: &Type) -> bool {
    let elem = match list_ty {
        Type::List(e) => e.as_ref(),
        _ => return false,
    };
    if !is_par_scalar(elem) || !is_par_scalar(init_ty) {
        return false;
    }
    match fun_ty {
        Type::Fun(params, out, eff) => {
            if eff.has_io() || !is_par_scalar(out) {
                return false;
            }
            if params.len() != 2 {
                return false;
            }
            is_par_scalar(&params[0]) && is_par_scalar(&params[1])
        }
        _ => false,
    }
}

/// Keep or demote `ListParMap` after inference (DESIGN: transparent auto-parallel).
///
/// - `enabled`: demote when impure / non-scalar; keep when eligible.
/// - `!enabled` (`--no-parallel`): demote every `ListParMap`.
pub fn finalize_auto_parallel(typed: &mut TypedModule, enabled: bool) {
    for item in &mut typed.module.items {
        if let Item::Fun(f) = item {
            finalize_par_maps_in_expr(&mut f.body, &typed.type_at, enabled);
        } else if let Item::Val { body, .. } = item {
            finalize_par_maps_in_expr(body, &typed.type_at, enabled);
        }
    }
}

pub(crate) fn finalize_par_maps_in_expr(
    expr: &mut Expr,
    type_at: &[(lumi_syntax::Span, Type)],
    enabled: bool,
) {
    match expr {
        Expr::BuiltinCall { name, args, span } => {
            for a in args.iter_mut() {
                finalize_par_maps_in_expr(a, type_at, enabled);
            }
            if matches!(name, Builtin::ListParMap) && args.len() == 2 {
                let keep = enabled
                    && type_at_span(type_at, expr_span(&args[0]))
                        .zip(type_at_span(type_at, expr_span(&args[1])))
                        .is_some_and(|(lt, ft)| list_par_map_eligible(&lt, &ft));
                if !keep {
                    let list = args[0].clone();
                    let f = args[1].clone();
                    let sp = *span;
                    *expr = desugar_list_map_sequential(&LowerCtx::empty(), list, f, sp);
                    finalize_par_maps_in_expr(expr, type_at, enabled);
                }
            } else if matches!(name, Builtin::ListParFold) && args.len() == 3 {
                let keep = enabled
                    && type_at_span(type_at, expr_span(&args[0]))
                        .zip(type_at_span(type_at, expr_span(&args[1])))
                        .zip(type_at_span(type_at, expr_span(&args[2])))
                        .is_some_and(|((lt, it), ft)| list_par_fold_eligible(&lt, &it, &ft));
                if !keep {
                    let list = args[0].clone();
                    let init = args[1].clone();
                    let f = args[2].clone();
                    let sp = *span;
                    *expr = desugar_list_fold_sequential(&LowerCtx::empty(), list, init, f, sp);
                    finalize_par_maps_in_expr(expr, type_at, enabled);
                }
            }
        }
        Expr::AdtNew { args, .. } => {
            for a in args {
                finalize_par_maps_in_expr(a, type_at, enabled);
            }
        }
        Expr::Let { value, body, .. } => {
            finalize_par_maps_in_expr(value, type_at, enabled);
            finalize_par_maps_in_expr(body, type_at, enabled);
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => {
            finalize_par_maps_in_expr(value, type_at, enabled);
        }
        Expr::Lambda { body, .. } => finalize_par_maps_in_expr(body, type_at, enabled),
        Expr::Call { callee, args, .. } => {
            finalize_par_maps_in_expr(callee, type_at, enabled);
            for a in args {
                finalize_par_maps_in_expr(a, type_at, enabled);
            }
        }
        Expr::Binary { left, right, .. } => {
            finalize_par_maps_in_expr(left, type_at, enabled);
            finalize_par_maps_in_expr(right, type_at, enabled);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            finalize_par_maps_in_expr(cond, type_at, enabled);
            finalize_par_maps_in_expr(then_branch, type_at, enabled);
            finalize_par_maps_in_expr(else_branch, type_at, enabled);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            finalize_par_maps_in_expr(cond, type_at, enabled);
            finalize_par_maps_in_expr(body, type_at, enabled);
            if let Some(s) = step {
                finalize_par_maps_in_expr(s, type_at, enabled);
            }
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                finalize_par_maps_in_expr(s, type_at, enabled);
            }
        }
        Expr::Return { value, .. } => finalize_par_maps_in_expr(value, type_at, enabled),
        Expr::Alt { scrutinee, alt, .. } => {
            finalize_par_maps_in_expr(scrutinee, type_at, enabled);
            finalize_par_maps_in_expr(alt, type_at, enabled);
        }
        Expr::With { base, fields, .. } => {
            finalize_par_maps_in_expr(base, type_at, enabled);
            for (_, e) in fields {
                finalize_par_maps_in_expr(e, type_at, enabled);
            }
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..)
        | Expr::Break(_)
        | Expr::Continue(_) => {}
    }
}

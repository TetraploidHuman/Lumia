//! Auto-parallel eligibility finalization.

use crate::types::{expr_span, Type, TypedModule};
use lumia_hir::{desugar_list_fold_sequential, desugar_list_map_sequential, Builtin, Expr, Item};

pub(crate) fn is_par_scalar(t: &Type) -> bool {
    matches!(t, Type::Int | Type::Bool | Type::Float)
}

pub fn type_at_span(
    type_at: &[(lumia_syntax::Span, Type)],
    span: lumia_syntax::Span,
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

/// Keep or demote `ListParMap` / `ListParFold` after inference.
///
/// - `enabled`: demote when impure / non-scalar; keep when eligible.
/// - `!enabled` (`--no-parallel`): demote every `ListPar*`.
/// - Inside `spawn` / `scope` (Task runtime): always demote — DESIGN forbids
///   mixing OS par workers with the fiber scheduler (RT would silently sequentialize).
pub fn finalize_auto_parallel(typed: &mut TypedModule, enabled: bool) {
    for item in &mut typed.module.items {
        if let Item::Fun(f) = item {
            finalize_par_maps_rec(&mut f.body, &typed.type_at, enabled, 0);
        } else if let Item::Val { body, .. } = item {
            finalize_par_maps_rec(body, &typed.type_at, enabled, 0);
        }
    }
}

/// `task_depth`: nesting under `TaskSpawn` thunks and `scope` Seq regions
/// (`ScopeEnter` … `ScopeLeave`).
fn finalize_par_maps_rec(
    expr: &mut Expr,
    type_at: &[(lumia_syntax::Span, Type)],
    enabled: bool,
    task_depth: u32,
) {
    match expr {
        Expr::Let { value, body, .. } => {
            finalize_par_maps_rec(value, type_at, enabled, task_depth);
            finalize_par_maps_rec(body, type_at, enabled, task_depth);
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => {
            finalize_par_maps_rec(value, type_at, enabled, task_depth);
        }
        Expr::Lambda { body, .. } => finalize_par_maps_rec(body, type_at, enabled, task_depth),
        Expr::Call { callee, args, .. } => {
            finalize_par_maps_rec(callee, type_at, enabled, task_depth);
            for a in args {
                finalize_par_maps_rec(a, type_at, enabled, task_depth);
            }
        }
        Expr::Binary { left, right, .. } => {
            finalize_par_maps_rec(left, type_at, enabled, task_depth);
            finalize_par_maps_rec(right, type_at, enabled, task_depth);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            finalize_par_maps_rec(cond, type_at, enabled, task_depth);
            finalize_par_maps_rec(then_branch, type_at, enabled, task_depth);
            finalize_par_maps_rec(else_branch, type_at, enabled, task_depth);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            finalize_par_maps_rec(cond, type_at, enabled, task_depth);
            finalize_par_maps_rec(body, type_at, enabled, task_depth);
            if let Some(s) = step {
                finalize_par_maps_rec(s, type_at, enabled, task_depth);
            }
        }
        Expr::Seq { stmts, .. } => {
            let mut depth = task_depth;
            for s in stmts {
                match s {
                    Expr::BuiltinCall {
                        name: Builtin::ScopeEnter,
                        ..
                    } => {
                        finalize_par_maps_rec(s, type_at, enabled, depth);
                        depth = depth.saturating_add(1);
                    }
                    Expr::BuiltinCall {
                        name: Builtin::ScopeLeave,
                        ..
                    } => {
                        finalize_par_maps_rec(s, type_at, enabled, depth);
                        depth = depth.saturating_sub(1);
                    }
                    _ => finalize_par_maps_rec(s, type_at, enabled, depth),
                }
            }
        }
        Expr::BuiltinCall { name, args, .. } => {
            let child_depth = match name {
                Builtin::TaskSpawn => task_depth.saturating_add(1),
                _ => task_depth,
            };
            for a in args.iter_mut() {
                finalize_par_maps_rec(a, type_at, enabled, child_depth);
            }
            demote_par_call(expr, type_at, enabled, task_depth);
        }
        Expr::AdtNew { args, .. } => {
            for a in args {
                finalize_par_maps_rec(a, type_at, enabled, task_depth);
            }
        }
        Expr::Return { value, .. } => {
            finalize_par_maps_rec(value, type_at, enabled, task_depth);
        }
        Expr::Alt { scrutinee, alt, .. } => {
            finalize_par_maps_rec(scrutinee, type_at, enabled, task_depth);
            finalize_par_maps_rec(alt, type_at, enabled, task_depth);
        }
        Expr::With { base, fields, .. } => {
            finalize_par_maps_rec(base, type_at, enabled, task_depth);
            for (_, e) in fields {
                finalize_par_maps_rec(e, type_at, enabled, task_depth);
            }
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(_)
        | Expr::Var(_, _)
        | Expr::Break(_)
        | Expr::Continue(_) => {}
    }
}

fn demote_par_call(
    expr: &mut Expr,
    type_at: &[(lumia_syntax::Span, Type)],
    enabled: bool,
    task_depth: u32,
) {
    let Expr::BuiltinCall { name, args, span } = expr else {
        return;
    };
    let under_task = task_depth > 0;
    match name {
        Builtin::ListParMap if args.len() == 2 => {
            let keep = enabled
                && !under_task
                && type_at_span(type_at, expr_span(&args[0]))
                    .zip(type_at_span(type_at, expr_span(&args[1])))
                    .is_some_and(|(lt, ft)| list_par_map_eligible(&lt, &ft));
            if !keep {
                let list = args[0].clone();
                let f = args[1].clone();
                let sp = *span;
                *expr = desugar_list_map_sequential(list, f, sp);
            }
        }
        Builtin::ListParFold if args.len() == 3 => {
            let keep = enabled
                && !under_task
                && type_at_span(type_at, expr_span(&args[0]))
                    .zip(type_at_span(type_at, expr_span(&args[1])))
                    .zip(type_at_span(type_at, expr_span(&args[2])))
                    .is_some_and(|((lt, it), ft)| list_par_fold_eligible(&lt, &it, &ft));
            if !keep {
                let list = args[0].clone();
                let init = args[1].clone();
                let f = args[2].clone();
                let sp = *span;
                *expr = desugar_list_fold_sequential(list, init, f, sp);
            }
        }
        _ => {}
    }
}

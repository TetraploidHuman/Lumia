//! Auto-parallel eligibility finalization.

use crate::types::{expr_span, Type, TypedModule};
use lumia_hir::{
    desugar_list_fold_sequential, desugar_list_map_sequential, for_each_expr_mut, Builtin, Expr,
    Item,
};

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
    type_at: &[(lumia_syntax::Span, Type)],
    enabled: bool,
) {
    // Post-order: children first. Sequential desugar never reintroduces ListPar*.
    for_each_expr_mut(expr, &mut |e| demote_par_call(e, type_at, enabled));
}

fn demote_par_call(expr: &mut Expr, type_at: &[(lumia_syntax::Span, Type)], enabled: bool) {
    let Expr::BuiltinCall { name, args, span } = expr else {
        return;
    };
    match name {
        Builtin::ListParMap if args.len() == 2 => {
            let keep = enabled
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

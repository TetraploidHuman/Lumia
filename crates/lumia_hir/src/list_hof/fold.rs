//! List fold desugaring (incl. parallel / range).

use super::{list_accum, range_accum, resolve_binary_callback, with_fun_bind, BinaryCallback};
use crate::ast::{Builtin, Expr};
use crate::lower::LowerCtx;
use crate::visit::free_vars_expr;
use lumia_syntax::{BinOp, Span};

pub(crate) fn lower_list_fold(ctx: &LowerCtx, list: Expr, init: Expr, f: Expr, span: Span) -> Expr {
    // `range(a,b).fold(...)` → counter loop (no HeapList materialization).
    if let Expr::BuiltinCall { name, args, .. } = &list {
        if matches!(name, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
            let inclusive = matches!(name, Builtin::RangeInclusive);
            let start = args[0].clone();
            let end = args[1].clone();
            return match resolve_binary_callback(f, span, "fold") {
                BinaryCallback::Inline {
                    acc,
                    acc_ty,
                    x,
                    x_ty,
                    body,
                } => range_fold_inline(
                    start, end, inclusive, init, acc, acc_ty, x, x_ty, body, span,
                ),
                BinaryCallback::Bound { f, f_name, acc, x } => {
                    range_fold_call(start, end, inclusive, init, f, f_name, acc, x, span)
                }
            };
        }
    }
    // FunRef-safe list fold → parallel candidate (associativity assumed).
    if fold_callback_is_parallel_safe(ctx, &f) {
        return Expr::BuiltinCall {
            name: Builtin::ListParFold,
            args: vec![list, init, f],
            span,
        };
    }
    desugar_list_fold_sequential(list, init, f, span)
}

/// Sequential `fold` loop (also used when auto-parallel demotes `ListParFold`).
pub fn desugar_list_fold_sequential(
    list: Expr,
    init: Expr,
    f: Expr,
    span: Span,
) -> Expr {
    match resolve_binary_callback(f, span, "fold") {
        BinaryCallback::Inline {
            acc,
            acc_ty,
            x,
            x_ty,
            body,
        } => lower_list_fold_inline(list, init, acc, acc_ty, x, x_ty, body, span),
        BinaryCallback::Bound { f, f_name, acc, x } => {
            lower_list_fold_call(list, init, f, f_name, acc, x, span)
        }
    }
}

/// Parallel fold: FunRef-safe **and** syntactically associative (`+` / `*`).
/// DESIGN: auto-parallel must not change values; non-associative ops (e.g. `-`)
/// yield wrong results under chunked combine.
fn fold_callback_is_parallel_safe(ctx: &LowerCtx, f: &Expr) -> bool {
    match f {
        Expr::Lambda { params, body, .. } if params.len() == 2 => {
            let bound: Vec<String> = params.clone();
            let frees = free_vars_expr(body, &bound);
            if !frees.iter().all(|n| ctx.is_toplevel_fun(n)) {
                return false;
            }
            fold_body_is_associative(body.as_ref(), &params[0], &params[1])
        }
        Expr::Var(n, _) => {
            if !ctx.is_toplevel_fun(n) {
                return false;
            }
            // Resolve top-level lambda body when registered as a val/fun binding.
            ctx.is_toplevel_fold_assoc(n)
        }
        _ => false,
    }
}

fn vars_are_fold_params(l: &str, r: &str, a: &str, b: &str) -> bool {
    (l == a && r == b) || (l == b && r == a)
}

trait FoldAssocExpr {
    fn match_add_mul_vars(&self, a: &str, b: &str) -> bool;
    fn nested_for_assoc(&self) -> Option<&Self>;
}

impl FoldAssocExpr for Expr {
    fn match_add_mul_vars(&self, a: &str, b: &str) -> bool {
        match self {
            Expr::Binary {
                op: BinOp::Add | BinOp::Mul,
                left,
                right,
                ..
            } => match (left.as_ref(), right.as_ref()) {
                (Expr::Var(l, _), Expr::Var(r, _)) => vars_are_fold_params(l, r, a, b),
                _ => false,
            },
            _ => false,
        }
    }

    fn nested_for_assoc(&self) -> Option<&Self> {
        match self {
            Expr::Seq { stmts, .. } => stmts.last(),
            Expr::Let { body, .. } => Some(body),
            _ => None,
        }
    }
}

impl FoldAssocExpr for lumia_syntax::Expr {
    fn match_add_mul_vars(&self, a: &str, b: &str) -> bool {
        match self {
            lumia_syntax::Expr::Binary {
                op: BinOp::Add | BinOp::Mul,
                left,
                right,
                ..
            } => match (left.as_ref(), right.as_ref()) {
                (lumia_syntax::Expr::Ident(l, _), lumia_syntax::Expr::Ident(r, _)) => {
                    vars_are_fold_params(l, r, a, b)
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn nested_for_assoc(&self) -> Option<&Self> {
        match self {
            lumia_syntax::Expr::Block { tail, .. } => tail.as_deref(),
            lumia_syntax::Expr::Lambda { body, .. } => Some(body),
            _ => None,
        }
    }
}

fn fold_body_is_associative<E: FoldAssocExpr>(body: &E, a: &str, b: &str) -> bool {
    if body.match_add_mul_vars(a, b) {
        return true;
    }
    body.nested_for_assoc()
        .is_some_and(|inner| fold_body_is_associative(inner, a, b))
}

/// Syntax-level twin of [`fold_body_is_associative`] (used while scanning items).
pub(crate) fn syntax_fold_body_is_associative(body: &lumia_syntax::Expr, a: &str, b: &str) -> bool {
    fold_body_is_associative(body, a, b)
}

fn range_fold_inline(
    start: Expr,
    end: Expr,
    inclusive: bool,
    init: Expr,
    acc: String,
    acc_ty: Option<String>,
    x: String,
    x_ty: Option<String>,
    body: Expr,
    span: Span,
) -> Expr {
    let el = format!("{}{}", crate::desugar_slots::FOLD_ELEM_PREFIX, span.start.0);
    let mut body = Expr::Let {
        name: x,
        value: Box::new(Expr::Var(el.clone(), span)),
        body: Box::new(body),
        mutable: false,
        ty: x_ty,
    };
    if let Some(ty) = acc_ty {
        body = Expr::Let {
            name: format!("__fold_acc_ann_{}", span.start.0),
            value: Box::new(Expr::Var(acc.clone(), span)),
            body: Box::new(body),
            mutable: false,
            ty: Some(ty),
        };
    }
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(body),
        span,
    };
    range_accum(acc, init, &el, start, end, inclusive, step, span)
}

fn range_fold_call(
    start: Expr,
    end: Expr,
    inclusive: bool,
    init: Expr,
    f: Expr,
    f_name: String,
    acc: String,
    x: String,
    span: Span,
) -> Expr {
    let applied = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(applied),
        span,
    };
    with_fun_bind(
        Some((f_name, f)),
        range_accum(acc, init, &x, start, end, inclusive, step, span),
    )
}

fn lower_list_fold_inline(
    list: Expr,
    init: Expr,
    acc: String,
    acc_ty: Option<String>,
    x: String,
    x_ty: Option<String>,
    body: Expr,
    span: Span,
) -> Expr {
    let el = format!("{}{}", crate::desugar_slots::FOLD_ELEM_PREFIX, span.start.0);
    let mut body = Expr::Let {
        name: x,
        value: Box::new(Expr::Var(el.clone(), span)),
        body: Box::new(body),
        mutable: false,
        ty: x_ty,
    };
    if let Some(ty) = acc_ty {
        body = Expr::Let {
            name: format!("__fold_acc_ann_{}", span.start.0),
            value: Box::new(Expr::Var(acc.clone(), span)),
            body: Box::new(body),
            mutable: false,
            ty: Some(ty),
        };
    }
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(body),
        span,
    };
    list_accum(acc, init, &el, list, step, span)
}

fn lower_list_fold_call(
    list: Expr,
    init: Expr,
    f: Expr,
    f_name: String,
    acc: String,
    x: String,
    span: Span,
) -> Expr {
    let applied = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(applied),
        span,
    };
    with_fun_bind(
        Some((f_name, f)),
        list_accum(acc, init, &x, list, step, span),
    )
}

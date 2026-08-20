//! Match exhaustiveness checking and pattern desugaring helpers.

mod exhaust;
mod pattern;

use crate::ast::{AdtDef, CtorInfo, Expr};
use crate::lower::LowerError;
use lumia_syntax::Span;
use rustc_hash::FxHashMap as HashMap;

pub(crate) use exhaust::check_match_exhaustiveness;
pub(crate) use pattern::{pattern_cond, pattern_irrefutable};

pub(crate) fn short_and(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(left),
        then_branch: Box::new(right),
        else_branch: Box::new(Expr::Bool(false, span)),
        span,
    }
}

pub(crate) fn short_or(left: Expr, right: Expr, span: Span) -> Expr {
    Expr::If {
        cond: Box::new(left),
        then_branch: Box::new(Expr::Bool(true, span)),
        else_branch: Box::new(right),
        span,
    }
}

pub(crate) fn check_module_matches(
    m: &lumia_syntax::Module,
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), LowerError> {
    for item in &m.items {
        match item {
            lumia_syntax::Item::Val(v) => {
                check_expr_matches(&v.body, ctors, adts, products)?;
            }
            lumia_syntax::Item::Trait(t) => {
                for method in &t.methods {
                    check_expr_matches(&method.body, ctors, adts, products)?;
                }
            }
            lumia_syntax::Item::Instance(i) => {
                for method in &i.methods {
                    check_expr_matches(&method.body, ctors, adts, products)?;
                }
            }
            lumia_syntax::Item::Type(_) | lumia_syntax::Item::Foreign(_) => {}
        }
    }
    Ok(())
}

pub(crate) fn check_expr_matches(
    e: &lumia_syntax::Expr,
    ctors: &HashMap<String, CtorInfo>,
    adts: &[AdtDef],
    products: &HashMap<String, Vec<String>>,
) -> Result<(), LowerError> {
    use lumia_syntax::Expr as S;
    match e {
        S::Match { arms, span, .. } => {
            check_match_exhaustiveness(arms, ctors, adts, products).map_err(|mut e| {
                e.span = *span;
                e
            })?;
            for a in arms {
                check_expr_matches(&a.body, ctors, adts, products)?;
                if let Some(g) = &a.guard {
                    check_expr_matches(g, ctors, adts, products)?;
                }
            }
        }
        S::MatchCond { arms, span, .. } => {
            if !arms.iter().any(|a| a.cond.is_none()) {
                return Err(LowerError {
                    message: "subjectless `match { }` used as expression requires a `_` arm".into(),
                    span: *span,
                });
            }
            // `_` must be last (Kotlin else is last)
            if let Some((last, rest)) = arms.split_last() {
                if last.cond.is_some() || rest.iter().any(|a| a.cond.is_none()) {
                    return Err(LowerError {
                        message: "subjectless `match { }`: `_` arm must be last and unique".into(),
                        span: *span,
                    });
                }
            }
            for a in arms {
                if let Some(c) = &a.cond {
                    check_expr_matches(c, ctors, adts, products)?;
                }
                check_expr_matches(&a.body, ctors, adts, products)?;
            }
        }
        S::Block { stmts, tail, .. } => {
            for s in stmts {
                match s {
                    lumia_syntax::Stmt::Val { expr, .. }
                    | lumia_syntax::Stmt::Var { expr, .. }
                    | lumia_syntax::Stmt::Assign { expr, .. }
                    | lumia_syntax::Stmt::Expr(expr) => {
                        check_expr_matches(expr, ctors, adts, products)?
                    }
                    lumia_syntax::Stmt::ForIn { iter, body, .. }
                    | lumia_syntax::Stmt::ForCond {
                        cond: iter, body, ..
                    } => {
                        check_expr_matches(iter, ctors, adts, products)?;
                        check_expr_matches(body, ctors, adts, products)?;
                    }
                    lumia_syntax::Stmt::Break(_) | lumia_syntax::Stmt::Continue(_) => {}
                }
            }
            if let Some(t) = tail {
                check_expr_matches(t, ctors, adts, products)?;
            }
        }
        S::Lambda { body, .. } => check_expr_matches(body, ctors, adts, products)?,
        S::Call { callee, args, .. } => {
            check_expr_matches(callee, ctors, adts, products)?;
            for a in args {
                check_expr_matches(a, ctors, adts, products)?;
            }
        }
        S::Binary { left, right, .. } | S::Pipeline { left, right, .. } => {
            check_expr_matches(left, ctors, adts, products)?;
            check_expr_matches(right, ctors, adts, products)?;
        }
        S::Unary { expr, .. } | S::Field { base: expr, .. } | S::Return { value: expr, .. } => {
            check_expr_matches(expr, ctors, adts, products)?
        }
        S::Alt { scrutinee, alt, .. } => {
            check_expr_matches(scrutinee, ctors, adts, products)?;
            check_expr_matches(alt, ctors, adts, products)?;
        }
        S::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_expr_matches(cond, ctors, adts, products)?;
            check_expr_matches(then_branch, ctors, adts, products)?;
            if let Some(e) = else_branch {
                check_expr_matches(e, ctors, adts, products)?;
            }
        }
        S::ListLit { elems, .. } => {
            for a in elems {
                check_expr_matches(a, ctors, adts, products)?;
            }
        }
        S::StructLit { fields, .. } => {
            for (_, v) in fields {
                check_expr_matches(v, ctors, adts, products)?;
            }
        }
        S::With { base, fields, .. } => {
            check_expr_matches(base, ctors, adts, products)?;
            for (_, v) in fields {
                check_expr_matches(v, ctors, adts, products)?;
            }
        }
        S::TupleLit { elems, .. } => {
            for a in elems {
                check_expr_matches(a, ctors, adts, products)?;
            }
        }
        S::Int(..) | S::Float(..) | S::Bool(..) | S::String(..) | S::Char(..) | S::Ident(..) => {}
        S::Interp { parts, .. } => {
            for p in parts {
                if let lumia_syntax::InterpPart::Expr(e) = p {
                    check_expr_matches(e, ctors, adts, products)?;
                }
            }
        }
        S::Scope {
            scheduler, body, ..
        } => {
            if let Some(s) = scheduler {
                check_expr_matches(s, ctors, adts, products)?;
            }
            check_expr_matches(body, ctors, adts, products)?;
        }
        S::Spawn { body, .. } => {
            check_expr_matches(body, ctors, adts, products)?;
        }
    }
    Ok(())
}

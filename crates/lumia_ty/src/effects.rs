//! Effect boundary checks after inference.
//!
//! ## Pure vs nested lambda (DESIGN §3.7)
//!
//! - **Constructing** a closure is always a pure expression: the Fun type carries `ε`.
//!   A pure function may build `{ -> println(...) }` and return / store it.
//! - **Calling** an effectful Fun (named or let-bound) requires an effect context
//!   (`main`, effectful function body, or the body of a Fun that itself carries IO).
//! - `assert_no_effects_in_pure` walks into lambda bodies under an *effect* context
//!   (so deferred IO is allowed) while still rejecting eager IO in the pure outer body.
//!   Let-bound locals that are syntactically IO thunks are tracked so `f()` in a pure
//!   function is rejected even when `f` is not in `fun_types`.

use crate::types::{at, Type, TypeError, TypedModule};
use lumia_hir::{Expr, Item};
use rustc_hash::FxHashMap as HashMap;

/// Reject calling effectful functions from pure contexts (simplified whole-program check).
pub fn check_effect_boundaries(typed: &TypedModule) -> Result<(), TypeError> {
    for item in &typed.module.items {
        if let Item::Fun(f) = item {
            let fun_ty = typed.fun_types.get(&f.name);
            let fun_is_effectful = match fun_ty {
                Some(Type::Fun(_, _, e)) => e.has_io() || f.is_main,
                _ => f.is_main,
            };
            // If inference claims pure, body must not contain any eager effect.
            if !fun_is_effectful {
                assert_no_effects_in_pure(&f.body, &typed.fun_types, &mut HashMap::default())?;
            }
            check_expr_effects(
                &f.body,
                fun_is_effectful,
                &typed.fun_types,
                &mut HashMap::default(),
            )?;
        }
    }
    Ok(())
}

/// When this Fun value is *called*, does its body perform IO?
/// Nested lambda *construction* is not IO; calling a nested IO thunk is.
fn fun_body_has_io(body: &Expr, fun_types: &HashMap<String, Type>) -> bool {
    fn walk(
        expr: &Expr,
        fun_types: &HashMap<String, Type>,
        locals: &mut HashMap<String, bool>,
    ) -> bool {
        match expr {
            Expr::BuiltinCall { name, args, .. } => {
                if name.is_io() {
                    return true;
                }
                args.iter().any(|a| walk(a, fun_types, locals))
            }
            Expr::Call { callee, args, .. } => {
                if let Expr::Var(name, _) = callee.as_ref() {
                    if locals.get(name).copied().unwrap_or(false) {
                        return true;
                    }
                    if let Some(Type::Fun(_, _, e)) = fun_types.get(name) {
                        if e.has_io() {
                            return true;
                        }
                    }
                }
                walk(callee, fun_types, locals) || args.iter().any(|a| walk(a, fun_types, locals))
            }
            Expr::Let {
                name, value, body, .. } => {
                if walk(value, fun_types, locals) {
                    return true;
                }
                let io = match value.as_ref() {
                    Expr::Lambda { body: lam, .. } => fun_body_has_io(lam, fun_types),
                    Expr::Var(n, _) => {
                        locals.get(n).copied().unwrap_or(false)
                            || matches!(
                                fun_types.get(n),
                                Some(Type::Fun(_, _, e)) if e.has_io()
                            )
                    }
                    _ => false,
                };
                let prev = locals.insert(name.clone(), io);
                let r = walk(body, fun_types, locals);
                match prev {
                    Some(v) => {
                        locals.insert(name.clone(), v);
                    }
                    None => {
                        locals.remove(name);
                    }
                }
                r
            }
            // Building a nested closure does not run its body.
            Expr::Lambda { .. } => false,
            Expr::Seq { stmts, .. } => stmts.iter().any(|s| walk(s, fun_types, locals)),
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                walk(cond, fun_types, locals)
                    || walk(then_branch, fun_types, locals)
                    || walk(else_branch, fun_types, locals)
            }
            Expr::Loop {
                cond, body, step, ..
            } => {
                walk(cond, fun_types, locals)
                    || walk(body, fun_types, locals)
                    || step.as_ref().is_some_and(|s| walk(s, fun_types, locals))
            }
            Expr::Binary { left, right, .. } => {
                walk(left, fun_types, locals) || walk(right, fun_types, locals)
            }
            Expr::Unary { expr, .. }
            | Expr::Assign { value: expr, .. }
            | Expr::Return { value: expr, .. } => walk(expr, fun_types, locals),
            Expr::Alt { scrutinee, alt, .. } => {
                walk(scrutinee, fun_types, locals) || walk(alt, fun_types, locals)
            }
            Expr::With { base, fields, .. } => {
                walk(base, fun_types, locals)
                    || fields.iter().any(|(_, e)| walk(e, fun_types, locals))
            }
            Expr::AdtNew { args, .. } => args.iter().any(|a| walk(a, fun_types, locals)),
            _ => false,
        }
    }
    walk(body, fun_types, &mut HashMap::default())
}

pub(crate) fn assert_no_effects_in_pure(
    expr: &Expr,
    fun_types: &HashMap<String, Type>,
    locals: &mut HashMap<String, bool>,
) -> Result<(), TypeError> {
    match expr {
        Expr::BuiltinCall { name, args, span } => {
            if name.is_io() {
                return Err(at(*span, "effectful call not allowed in pure function"));
            }
            for a in args {
                assert_no_effects_in_pure(a, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Call { callee, args, span } => {
            if let Expr::Var(name, _) = callee.as_ref() {
                if locals.get(name).copied().unwrap_or(false) {
                    return Err(at(
                        *span,
                        format!("cannot call effectful `{name}` from pure function"),
                    ));
                }
                if let Some(Type::Fun(_, _, e)) = fun_types.get(name) {
                    if e.has_io() {
                        return Err(at(
                            *span,
                            format!("cannot call effectful `{name}` from pure function"),
                        ));
                    }
                }
            }
            assert_no_effects_in_pure(callee, fun_types, locals)?;
            for a in args {
                assert_no_effects_in_pure(a, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Let {
            name, value, body, .. } => {
            assert_no_effects_in_pure(value, fun_types, locals)?;
            let io = match value.as_ref() {
                Expr::Lambda { body: lam_body, .. } => fun_body_has_io(lam_body, fun_types),
                Expr::Var(n, _) => {
                    locals.get(n).copied().unwrap_or(false)
                        || matches!(
                            fun_types.get(n),
                            Some(Type::Fun(_, _, e)) if e.has_io()
                        )
                }
                _ => false,
            };
            let prev = locals.insert(name.clone(), io);
            let r = assert_no_effects_in_pure(body, fun_types, locals);
            match prev {
                Some(v) => {
                    locals.insert(name.clone(), v);
                }
                None => {
                    locals.remove(name);
                }
            }
            r
        }
        Expr::Assign { value, .. } => assert_no_effects_in_pure(value, fun_types, locals),
        Expr::Lambda { body, .. } => {
            // Construction is pure for the outer function. Enter the body as an
            // effect context so deferred IO is allowed; Fun type carries ε.
            check_expr_effects(body, true, fun_types, &mut HashMap::default())
        }
        Expr::Binary { left, right, .. } => {
            assert_no_effects_in_pure(left, fun_types, locals)?;
            assert_no_effects_in_pure(right, fun_types, locals)
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                assert_no_effects_in_pure(s, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } => assert_no_effects_in_pure(expr, fun_types, locals),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            assert_no_effects_in_pure(cond, fun_types, locals)?;
            assert_no_effects_in_pure(then_branch, fun_types, locals)?;
            assert_no_effects_in_pure(else_branch, fun_types, locals)
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            assert_no_effects_in_pure(cond, fun_types, locals)?;
            assert_no_effects_in_pure(body, fun_types, locals)?;
            if let Some(s) = step {
                assert_no_effects_in_pure(s, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Break(_) | Expr::Continue(_) => Ok(()),
        Expr::Return { value, .. } => assert_no_effects_in_pure(value, fun_types, locals),
        Expr::Alt { scrutinee, alt, .. } => {
            assert_no_effects_in_pure(scrutinee, fun_types, locals)?;
            assert_no_effects_in_pure(alt, fun_types, locals)
        }
        Expr::With { base, fields, .. } => {
            assert_no_effects_in_pure(base, fun_types, locals)?;
            for (_, e) in fields {
                assert_no_effects_in_pure(e, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::AdtNew { args, .. } => {
            for a in args {
                assert_no_effects_in_pure(a, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..) => Ok(()),
    }
}

pub(crate) fn check_expr_effects(
    expr: &Expr,
    in_effect_ctx: bool,
    fun_types: &HashMap<String, Type>,
    locals: &mut HashMap<String, bool>,
) -> Result<(), TypeError> {
    match expr {
        Expr::BuiltinCall { name, args, span } => {
            if name.is_io() && !in_effect_ctx {
                return Err(at(*span, "effectful call not allowed in pure context"));
            }
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Call { callee, args, span } => {
            if let Expr::Var(name, _) = callee.as_ref() {
                let local_io = locals.get(name).copied().unwrap_or(false);
                if local_io && !in_effect_ctx {
                    return Err(at(
                        *span,
                        format!("cannot call effectful `{name}` from pure context"),
                    ));
                }
                if let Some(Type::Fun(_, _, e)) = fun_types.get(name) {
                    if e.has_io() && !in_effect_ctx {
                        return Err(at(
                            *span,
                            format!("cannot call effectful `{name}` from pure context"),
                        ));
                    }
                }
            }
            check_expr_effects(callee, in_effect_ctx, fun_types, locals)?;
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Let {
            name, value, body, .. } => {
            check_expr_effects(value, in_effect_ctx, fun_types, locals)?;
            let io = match value.as_ref() {
                Expr::Lambda { body: lam_body, .. } => fun_body_has_io(lam_body, fun_types),
                Expr::Var(n, _) => {
                    locals.get(n).copied().unwrap_or(false)
                        || matches!(
                            fun_types.get(n),
                            Some(Type::Fun(_, _, e)) if e.has_io()
                        )
                }
                _ => false,
            };
            let prev = locals.insert(name.clone(), io);
            let r = check_expr_effects(body, in_effect_ctx, fun_types, locals);
            match prev {
                Some(v) => {
                    locals.insert(name.clone(), v);
                }
                None => {
                    locals.remove(name);
                }
            }
            r
        }
        Expr::Assign { value, .. } => check_expr_effects(value, in_effect_ctx, fun_types, locals),
        Expr::Lambda { body, .. } => {
            // Lambda bodies are their own effect context: effectful bodies are OK
            // (the Fun type carries ε); check under an effectful context.
            check_expr_effects(body, true, fun_types, &mut HashMap::default())
        }
        Expr::Binary { left, right, .. } => {
            check_expr_effects(left, in_effect_ctx, fun_types, locals)?;
            check_expr_effects(right, in_effect_ctx, fun_types, locals)
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                check_expr_effects(s, in_effect_ctx, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } => check_expr_effects(expr, in_effect_ctx, fun_types, locals),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_expr_effects(cond, in_effect_ctx, fun_types, locals)?;
            check_expr_effects(then_branch, in_effect_ctx, fun_types, locals)?;
            check_expr_effects(else_branch, in_effect_ctx, fun_types, locals)
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            check_expr_effects(cond, in_effect_ctx, fun_types, locals)?;
            check_expr_effects(body, in_effect_ctx, fun_types, locals)?;
            if let Some(s) = step {
                check_expr_effects(s, in_effect_ctx, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Break(_) | Expr::Continue(_) => Ok(()),
        Expr::Return { value, .. } => check_expr_effects(value, in_effect_ctx, fun_types, locals),
        Expr::Alt { scrutinee, alt, .. } => {
            check_expr_effects(scrutinee, in_effect_ctx, fun_types, locals)?;
            check_expr_effects(alt, in_effect_ctx, fun_types, locals)
        }
        Expr::With { base, fields, .. } => {
            check_expr_effects(base, in_effect_ctx, fun_types, locals)?;
            for (_, e) in fields {
                check_expr_effects(e, in_effect_ctx, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::AdtNew { args, .. } => {
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types, locals)?;
            }
            Ok(())
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..) => Ok(()),
    }
}

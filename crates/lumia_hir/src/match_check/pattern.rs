//! Pattern → condition / bind desugaring.

use super::{short_and, short_or};
use crate::ast::{Builtin, Expr};
use crate::lower::LowerCtx;
use lumia_syntax::{BinOp, Pattern, Span};

/// Last-arm elision: only `_` / binders (and all-irrefutable `or`) may skip the
/// tag test + `MatchFail`. Nullary ctor names like `None` are refutable — same
/// rule as [`super::coverage_catch_all`].
pub(crate) fn pattern_irrefutable(ctx: &LowerCtx, pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard(_) => true,
        Pattern::Ident(name, _) => ctx.lookup_ctor(name).is_none_or(|c| c.arity != 0),
        Pattern::Or(ps, _) => !ps.is_empty() && ps.iter().all(|p| pattern_irrefutable(ctx, p)),
        Pattern::Tuple { elems, .. } => elems.iter().all(|p| pattern_irrefutable(ctx, p)),
        Pattern::Struct { fields, .. } => fields.iter().all(|(_, p)| pattern_irrefutable(ctx, p)),
        // Variants / lists / constants are refutable — not allowed in `val` bindings.
        _ => false,
    }
}

/// Build match condition + binder equations for `pat` against scrutinee expression `scrut`.
/// Nested patterns compose field/get paths (no temps), so binders stay valid in the arm body.
pub(crate) fn pattern_cond(
    ctx: &LowerCtx,
    pat: &Pattern,
    scrut: &Expr,
    span: Span,
) -> (Expr, Vec<(String, Expr)>) {
    match pat {
        Pattern::Wildcard(_) => (Expr::Bool(true, span), vec![]),
        Pattern::Int(n, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Int(*n, *s)),
                span,
            },
            vec![],
        ),
        Pattern::Float(n, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Float(*n, *s)),
                span,
            },
            vec![],
        ),
        Pattern::Bool(b, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Bool(*b, *s)),
                span,
            },
            vec![],
        ),
        Pattern::Char(c, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::Char(*c, *s)),
                span,
            },
            vec![],
        ),
        Pattern::String(t, s) => (
            Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(scrut.clone()),
                right: Box::new(Expr::String(t.clone(), *s)),
                span,
            },
            vec![],
        ),
        Pattern::Ident(name, _) => {
            if let Some(c) = ctx.lookup_ctor(name) {
                if c.arity == 0 {
                    let tag = Expr::BuiltinCall {
                        name: Builtin::AdtTag,
                        args: vec![scrut.clone()],
                        span,
                    };
                    return (
                        Expr::Binary {
                            op: BinOp::Eq,
                            left: Box::new(tag),
                            right: Box::new(Expr::Int(c.tag, span)),
                            span,
                        },
                        vec![],
                    );
                }
            }
            (Expr::Bool(true, span), vec![(name.clone(), scrut.clone())])
        }
        Pattern::Or(pats, _) => {
            // Nested or-patterns with binders are ambiguous; top-level or is expanded.
            let mut cond = Expr::Bool(false, span);
            let mut binds = vec![];
            for p in pats {
                let (c, b) = pattern_cond(ctx, p, scrut, span);
                if !b.is_empty() {
                    ctx.set_err(
                        "nested or-pattern with bindings is not supported; use separate match arms"
                            .into(),
                        span,
                    );
                }
                if binds.is_empty() {
                    binds = b;
                }
                cond = short_or(cond, c, span);
            }
            (cond, binds)
        }
        Pattern::Variant { name, args, .. } => {
            let Some(c) = ctx.lookup_ctor(name) else {
                ctx.set_err(format!("unknown variant `{name}` in pattern"), span);
                return (Expr::Bool(false, span), vec![]);
            };
            if args.len() != c.arity {
                ctx.set_err(
                    format!(
                        "variant `{name}` expects {} field(s), pattern has {}",
                        c.arity,
                        args.len()
                    ),
                    span,
                );
            }
            let tag = Expr::BuiltinCall {
                name: Builtin::AdtTag,
                args: vec![scrut.clone()],
                span,
            };
            let mut cond = Expr::Binary {
                op: BinOp::Eq,
                left: Box::new(tag),
                right: Box::new(Expr::Int(c.tag, span)),
                span,
            };
            let mut binds = vec![];
            let nfields = args.len().min(c.arity);
            for (i, ep) in args.iter().take(nfields).enumerate() {
                // Result/Option: pass ctor so ty maps Ok→T / Err→E / Some→T.
                // Other sum ctors keep 2-arg field (params[idx]); product names
                // would incorrectly fail nominal checks if we passed the ctor.
                let mut field_args = vec![scrut.clone(), Expr::Int(i as i64, span)];
                if matches!(name.as_str(), "Ok" | "Err" | "Some") {
                    field_args.push(Expr::String(name.clone(), span));
                }
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: field_args,
                    span,
                };
                match ep {
                    Pattern::Ident(n, _) if ctx.lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        // Binder (not a nullary ctor name).
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &field, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            (cond, binds)
        }
        Pattern::Struct { name, fields, .. } => {
            let Some(order) = ctx.lookup_product(name) else {
                ctx.set_err(
                    format!("unknown product type `{name}` in struct pattern"),
                    span,
                );
                return (Expr::Bool(false, span), vec![]);
            };
            let mut cond = Expr::Bool(true, span);
            let mut binds = vec![];
            for (fname, sub) in fields {
                let Some(idx) = order.iter().position(|f| f == fname) else {
                    ctx.set_err(
                        format!("unknown field `{fname}` in `{name}` struct pattern"),
                        span,
                    );
                    continue;
                };
                // Nominal product name so ty rejects `Rect` matched as `Point`.
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![
                        scrut.clone(),
                        Expr::Int(idx as i64, span),
                        Expr::String(name.clone(), span),
                    ],
                    span,
                };
                match sub {
                    Pattern::Ident(n, _) if ctx.lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &field, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            (cond, binds)
        }
        Pattern::Tuple { elems, .. } => {
            let mut cond = Expr::Bool(true, span);
            let mut binds = vec![];
            for (i, ep) in elems.iter().enumerate() {
                let field = Expr::BuiltinCall {
                    name: Builtin::AdtField,
                    args: vec![scrut.clone(), Expr::Int(i as i64, span)],
                    span,
                };
                match ep {
                    Pattern::Ident(n, _) if ctx.lookup_ctor(n).is_none_or(|c| c.arity != 0) => {
                        binds.push((n.clone(), field));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &field, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            (cond, binds)
        }
        Pattern::List { elems, rest, .. } => {
            let len = Expr::BuiltinCall {
                name: Builtin::ListLen,
                args: vec![scrut.clone()],
                span,
            };
            let min = elems.len() as i64;
            let mut cond = if rest.is_some() {
                Expr::Binary {
                    op: BinOp::Ge,
                    left: Box::new(len),
                    right: Box::new(Expr::Int(min, span)),
                    span,
                }
            } else {
                Expr::Binary {
                    op: BinOp::Eq,
                    left: Box::new(len),
                    right: Box::new(Expr::Int(min, span)),
                    span,
                }
            };
            let mut binds = vec![];
            for (i, ep) in elems.iter().enumerate() {
                let get = Expr::BuiltinCall {
                    name: Builtin::ListGet,
                    args: vec![scrut.clone(), Expr::Int(i as i64, span)],
                    span,
                };
                match ep {
                    Pattern::Ident(name, _)
                        if ctx.lookup_ctor(name).is_none_or(|c| c.arity != 0) =>
                    {
                        binds.push((name.clone(), get));
                    }
                    Pattern::Wildcard(_) => {}
                    sub => {
                        let (sub_cond, sub_binds) = pattern_cond(ctx, sub, &get, span);
                        cond = short_and(cond, sub_cond, span);
                        binds.extend(sub_binds);
                    }
                }
            }
            if let Some(rname) = rest {
                let slice = Expr::BuiltinCall {
                    name: Builtin::ListSlice,
                    args: vec![scrut.clone(), Expr::Int(min, span)],
                    span,
                };
                binds.push((rname.clone(), slice));
            }
            (cond, binds)
        }
    }
}

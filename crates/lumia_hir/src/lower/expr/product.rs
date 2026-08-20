//! Struct literal and `with` lowering.

use super::super::ctx::LowerCtx;
use super::lower_expr;
use crate::ast::{Builtin, Expr};
use lumia_syntax::{Span, Sym};
use rustc_hash::FxHashMap as HashMap;

pub(super) fn lower_struct_lit(
    ctx: &LowerCtx,
    name: &Sym,
    fields: &[(Sym, lumia_syntax::Expr)],
    span: Span,
) -> Expr {
    let name_s = name.as_str();
    let Some(order) = ctx.lookup_product(name_s) else {
        return Expr::Call {
            callee: Box::new(Expr::Var(name.clone(), span)),
            args: fields.iter().map(|(_, e)| lower_expr(ctx, e)).collect(),
            span,
        };
    };
    let mut by_name: HashMap<String, Expr> = HashMap::default();
    for (f, e) in fields {
        if by_name.insert(f.to_string(), lower_expr(ctx, e)).is_some() {
            ctx.set_err(
                format!("duplicate field `{f}` in `{name_s}` struct literal"),
                span,
            );
        }
    }
    let mut args = Vec::with_capacity(order.len());
    for f in &order {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            ctx.set_err(
                format!("missing field `{f}` in `{name_s}` struct literal"),
                span,
            );
            args.push(Expr::Int(0, span));
        }
    }
    if let Some((extra, _)) = by_name.iter().next() {
        ctx.set_err(
            format!("unknown field `{extra}` in `{name_s}` struct literal"),
            span,
        );
    }
    Expr::AdtNew {
        adt_name: name.clone(),
        variant: name.clone(),
        tag: 0,
        args,
        span,
    }
}

pub(super) fn lower_with(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    fields: &[(Sym, lumia_syntax::Expr)],
    span: Span,
) -> Expr {
    if fields.is_empty() {
        return lower_expr(ctx, base);
    }
    Expr::With {
        base: Box::new(lower_expr(ctx, base)),
        fields: fields
            .iter()
            .map(|(n, e)| (n.clone(), lower_expr(ctx, e)))
            .collect(),
        span,
    }
}

/// Expand `base with { … }` once the product type is known (lower or ty rewrite).
pub fn expand_with_known(
    products: &HashMap<Sym, Vec<Sym>>,
    type_name: Sym,
    base: Expr,
    fields: Vec<(Sym, Expr)>,
    span: Span,
) -> Expr {
    let Some(order) = products.get(&type_name) else {
        return base;
    };
    let tmp = Sym::from(format!("__with_{}", span.start.0));
    let mut by_name: HashMap<Sym, Expr> = HashMap::default();
    for (f, e) in fields {
        by_name.insert(f, e);
    }
    let mut args = Vec::with_capacity(order.len());
    for (i, f) in order.iter().enumerate() {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            args.push(Expr::BuiltinCall {
                name: Builtin::AdtField,
                args: vec![
                    Expr::Var(tmp.clone(), span),
                    Expr::Int(i as i64, span),
                    Expr::String(type_name.clone(), span),
                ],
                span,
            });
        }
    }
    Expr::Let {
        name: tmp,
        value: Box::new(base),
        body: Box::new(Expr::AdtNew {
            adt_name: type_name,
            variant: Sym::from(""),
            tag: 0,
            args,
            span,
        }),
        mutable: false,
        ty: None,
    }
}

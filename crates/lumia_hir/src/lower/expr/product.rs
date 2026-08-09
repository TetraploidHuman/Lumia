//! Struct literal and `with` lowering.

use super::super::ctx::LowerCtx;
use super::lower_expr;
use crate::ast::{Builtin, Expr};
use lumia_syntax::Span;
use rustc_hash::FxHashMap as HashMap;

pub(super) fn lower_struct_lit(
    ctx: &LowerCtx,
    name: &str,
    fields: &[(String, lumia_syntax::Expr)],
    span: Span,
) -> Expr {
    let Some(order) = ctx.lookup_product(name) else {
        // Unknown product — leave as call-shaped fallback
        return Expr::Call {
            callee: Box::new(Expr::Var(name.into(), span)),
            args: fields.iter().map(|(_, e)| lower_expr(ctx, e)).collect(),
            span,
        };
    };
    let mut by_name: HashMap<String, Expr> = HashMap::default();
    for (f, e) in fields {
        if by_name.insert(f.clone(), lower_expr(ctx, e)).is_some() {
            ctx.set_err(
                format!("duplicate field `{f}` in `{name}` struct literal"),
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
                format!("missing field `{f}` in `{name}` struct literal"),
                span,
            );
            // Placeholder; `lower_module` aborts on LOWER_ERR.
            args.push(Expr::Int(0, span));
        }
    }
    if let Some((extra, _)) = by_name.iter().next() {
        ctx.set_err(
            format!("unknown field `{extra}` in `{name}` struct literal"),
            span,
        );
    }
    Expr::AdtNew {
        adt_name: name.into(),
        variant: name.into(),
        tag: 0,
        args,
        span,
    }
}

pub(super) fn lower_with(
    ctx: &LowerCtx,
    base: &lumia_syntax::Expr,
    fields: &[(String, lumia_syntax::Expr)],
    span: Span,
) -> Expr {
    // Infer product from first updated field name. Shared field names across
    // product types are stripped from the map (ambiguous) and must error here.
    let Some((fname, _)) = fields.first() else {
        return lower_expr(ctx, base);
    };
    let Some((type_name, _)) = ctx.lookup_product_field(fname) else {
        ctx.set_err(
            format!(
                "cannot resolve `with` field `{fname}` (unknown or ambiguous across product types)"
            ),
            span,
        );
        return lower_expr(ctx, base);
    };
    let Some(order) = ctx.lookup_product(&type_name) else {
        ctx.set_err(
            format!("unknown product type `{type_name}` in `with`"),
            span,
        );
        return lower_expr(ctx, base);
    };
    let base_e = lower_expr(ctx, base);
    // Bind base once
    let tmp = format!("__with_{}", span.start.0);
    let mut by_name: HashMap<String, Expr> = HashMap::default();
    for (f, e) in fields {
        by_name.insert(f.clone(), lower_expr(ctx, e));
    }
    let mut args = Vec::with_capacity(order.len());
    for (i, f) in order.iter().enumerate() {
        if let Some(e) = by_name.remove(f) {
            args.push(e);
        } else {
            args.push(Expr::BuiltinCall {
                name: Builtin::AdtField,
                args: vec![Expr::Var(tmp.clone(), span), Expr::Int(i as i64, span)],
                span,
            });
        }
    }
    Expr::Let {
        name: tmp,
        value: Box::new(base_e),
        body: Box::new(Expr::AdtNew {
            adt_name: type_name,
            variant: String::new(),
            tag: 0,
            args,
            span,
        }),
        mutable: false,
    }
}

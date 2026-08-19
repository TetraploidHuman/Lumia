//! Resolve ambiguous product fields / deferred `with` after inference.

use lumia_hir::{expand_with_known, for_each_expr_mut, Builtin, Expr, Item, Module};
use lumia_syntax::{Span, Sym};
use rustc_hash::FxHashMap as HashMap;

/// Rewrite unresolved `AdtField(_, -1, field)` and deferred `Expr::With`.
pub(crate) fn apply_product_field_rewrites(
    module: &mut Module,
    field_rewrites: &HashMap<Span, (Sym, i64)>,
    with_rewrites: &HashMap<Span, Sym>,
) {
    if field_rewrites.is_empty() && with_rewrites.is_empty() {
        return;
    }
    let products: HashMap<Sym, Vec<Sym>> = module
        .products
        .iter()
        .map(|p| (p.name.clone(), p.fields.clone()))
        .collect();
    for item in &mut module.items {
        match item {
            Item::Fun(f) => rewrite_expr(&mut f.body, field_rewrites, with_rewrites, &products),
            Item::Val { body, .. } => rewrite_expr(body, field_rewrites, with_rewrites, &products),
        }
    }
}

fn rewrite_expr(
    expr: &mut Expr,
    field_rewrites: &HashMap<Span, (Sym, i64)>,
    with_rewrites: &HashMap<Span, Sym>,
    products: &HashMap<Sym, Vec<Sym>>,
) {
    // Post-order: field/`with` payloads rewrite before the node itself expands.
    for_each_expr_mut(expr, &mut |e| {
        apply_node_rewrite(e, field_rewrites, with_rewrites, products);
    });
}

fn apply_node_rewrite(
    expr: &mut Expr,
    field_rewrites: &HashMap<Span, (Sym, i64)>,
    with_rewrites: &HashMap<Span, Sym>,
    products: &HashMap<Sym, Vec<Sym>>,
) {
    match expr {
        Expr::With { span, .. } => {
            let span = *span;
            if let Some(adt) = with_rewrites.get(&span) {
                let Expr::With { base, fields, span } = std::mem::replace(expr, Expr::Unit(span))
                else {
                    unreachable!()
                };
                // Expansion uses concrete field indices — no further With rewrite needed.
                *expr = expand_with_known(products, adt.clone(), *base, fields, span);
            }
        }
        Expr::BuiltinCall {
            name: Builtin::AdtField,
            args,
            span,
        } if args.len() == 3 => {
            if let Some((adt, idx)) = field_rewrites.get(span) {
                if matches!(&args[1], Expr::Int(-1, _)) {
                    args[1] = Expr::Int(*idx, *span);
                    args[2] = Expr::String(adt.clone(), *span);
                }
            }
        }
        _ => {}
    }
}

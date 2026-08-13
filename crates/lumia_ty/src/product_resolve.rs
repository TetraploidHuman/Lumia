//! Resolve ambiguous product fields / deferred `with` after inference.

use lumia_hir::{expand_with_known, Builtin, Expr, Item, Module};
use lumia_syntax::Span;
use rustc_hash::FxHashMap as HashMap;

/// Rewrite unresolved `AdtField(_, -1, field)` and deferred `Expr::With`.
pub(crate) fn apply_product_field_rewrites(
    module: &mut Module,
    field_rewrites: &HashMap<Span, (String, i64)>,
    with_rewrites: &HashMap<Span, String>,
) {
    if field_rewrites.is_empty() && with_rewrites.is_empty() {
        return;
    }
    let products: HashMap<String, Vec<String>> = module
        .products
        .iter()
        .map(|p| (p.name.clone(), p.fields.clone()))
        .collect();
    for item in &mut module.items {
        match item {
            Item::Fun(f) => rewrite_expr(&mut f.body, field_rewrites, with_rewrites, &products),
            Item::Val { body, .. } => {
                rewrite_expr(body, field_rewrites, with_rewrites, &products)
            }
        }
    }
}

fn rewrite_expr(
    expr: &mut Expr,
    field_rewrites: &HashMap<Span, (String, i64)>,
    with_rewrites: &HashMap<Span, String>,
    products: &HashMap<String, Vec<String>>,
) {
    match expr {
        Expr::With { base, fields, span } => {
            rewrite_expr(base, field_rewrites, with_rewrites, products);
            for (_, e) in fields.iter_mut() {
                rewrite_expr(e, field_rewrites, with_rewrites, products);
            }
            let span = *span;
            if let Some(adt) = with_rewrites.get(&span) {
                let Expr::With { base, fields, span } =
                    std::mem::replace(expr, Expr::Unit(span))
                else {
                    unreachable!()
                };
                *expr = expand_with_known(products, adt.clone(), *base, fields, span);
                rewrite_expr(expr, field_rewrites, with_rewrites, products);
            }
        }
        Expr::BuiltinCall {
            name: Builtin::AdtField,
            args,
            span,
        } if args.len() == 3 => {
            for a in args.iter_mut() {
                rewrite_expr(a, field_rewrites, with_rewrites, products);
            }
            if let Some((adt, idx)) = field_rewrites.get(span) {
                if matches!(&args[1], Expr::Int(-1, _)) {
                    args[1] = Expr::Int(*idx, *span);
                    args[2] = Expr::String(adt.clone(), *span);
                }
            }
        }
        Expr::Let { value, body, .. } => {
            rewrite_expr(value, field_rewrites, with_rewrites, products);
            rewrite_expr(body, field_rewrites, with_rewrites, products);
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } | Expr::Return { value, .. } => {
            rewrite_expr(value, field_rewrites, with_rewrites, products);
        }
        Expr::Lambda { body, .. } => rewrite_expr(body, field_rewrites, with_rewrites, products),
        Expr::Call { callee, args, .. } => {
            rewrite_expr(callee, field_rewrites, with_rewrites, products);
            for a in args {
                rewrite_expr(a, field_rewrites, with_rewrites, products);
            }
        }
        Expr::Binary { left, right, .. } => {
            rewrite_expr(left, field_rewrites, with_rewrites, products);
            rewrite_expr(right, field_rewrites, with_rewrites, products);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr(cond, field_rewrites, with_rewrites, products);
            rewrite_expr(then_branch, field_rewrites, with_rewrites, products);
            rewrite_expr(else_branch, field_rewrites, with_rewrites, products);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            rewrite_expr(cond, field_rewrites, with_rewrites, products);
            rewrite_expr(body, field_rewrites, with_rewrites, products);
            if let Some(s) = step {
                rewrite_expr(s, field_rewrites, with_rewrites, products);
            }
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                rewrite_expr(s, field_rewrites, with_rewrites, products);
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                rewrite_expr(a, field_rewrites, with_rewrites, products);
            }
        }
        Expr::Alt { scrutinee, alt, .. } => {
            rewrite_expr(scrutinee, field_rewrites, with_rewrites, products);
            rewrite_expr(alt, field_rewrites, with_rewrites, products);
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

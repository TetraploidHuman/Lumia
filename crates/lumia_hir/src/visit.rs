//! Shared walks over HIR [`Expr`] trees.

use crate::Expr;
use rustc_hash::FxHashSet as HashSet;

/// Visit every sub-expression in pre-order.
pub fn for_each_expr(expr: &Expr, f: &mut impl FnMut(&Expr)) {
    f(expr);
    match expr {
        Expr::Let { value, body, .. } => {
            for_each_expr(value, f);
            for_each_expr(body, f);
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => {
            for_each_expr(value, f);
        }
        Expr::Lambda { body, .. } => for_each_expr(body, f),
        Expr::Call { callee, args, .. } => {
            for_each_expr(callee, f);
            for_each_exprs(args, f);
        }
        Expr::Binary { left, right, .. } => {
            for_each_expr(left, f);
            for_each_expr(right, f);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            for_each_expr(cond, f);
            for_each_expr(then_branch, f);
            for_each_expr(else_branch, f);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            for_each_expr(cond, f);
            for_each_expr(body, f);
            if let Some(s) = step {
                for_each_expr(s, f);
            }
        }
        Expr::Seq { stmts, .. } => for_each_exprs(stmts, f),
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for_each_exprs(args, f);
        }
        Expr::Return { value, .. } => for_each_expr(value, f),
        Expr::Alt { scrutinee, alt, .. } => {
            for_each_expr(scrutinee, f);
            for_each_expr(alt, f);
        }
        Expr::With { base, fields, .. } => {
            for_each_expr(base, f);
            for (_, e) in fields {
                for_each_expr(e, f);
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

fn for_each_exprs(exprs: &[Expr], f: &mut impl FnMut(&Expr)) {
    for e in exprs {
        for_each_expr(e, f);
    }
}

/// Fold over an expression tree bottom-up.
pub fn fold<T>(expr: &Expr, init: T, f: impl FnMut(T, &Expr) -> T) -> T {
    fold_impl(expr, init, &mut { f })
}

fn fold_impl<T>(expr: &Expr, init: T, f: &mut impl FnMut(T, &Expr) -> T) -> T {
    let acc = match expr {
        Expr::Let { value, body, .. } => {
            let acc = fold_impl(value, init, f);
            fold_impl(body, acc, f)
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => fold_impl(value, init, f),
        Expr::Lambda { body, .. } => fold_impl(body, init, f),
        Expr::Call { callee, args, .. } => {
            let acc = fold_impl(callee, init, f);
            args.iter().fold(acc, |acc, e| fold_impl(e, acc, f))
        }
        Expr::Binary { left, right, .. } => {
            let acc = fold_impl(left, init, f);
            fold_impl(right, acc, f)
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let acc = fold_impl(cond, init, f);
            let acc = fold_impl(then_branch, acc, f);
            fold_impl(else_branch, acc, f)
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            let acc = fold_impl(cond, init, f);
            let acc = fold_impl(body, acc, f);
            match step {
                Some(s) => fold_impl(s, acc, f),
                None => acc,
            }
        }
        Expr::Seq { stmts, .. } => stmts.iter().fold(init, |acc, e| fold_impl(e, acc, f)),
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            args.iter().fold(init, |acc, e| fold_impl(e, acc, f))
        }
        Expr::Return { value, .. } => fold_impl(value, init, f),
        Expr::Alt { scrutinee, alt, .. } => {
            let acc = fold_impl(scrutinee, init, f);
            fold_impl(alt, acc, f)
        }
        Expr::With { base, fields, .. } => {
            let acc = fold_impl(base, init, f);
            fields.iter().fold(acc, |acc, (_, e)| fold_impl(e, acc, f))
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(_)
        | Expr::Var(_, _)
        | Expr::Break(_)
        | Expr::Continue(_) => init,
    };
    f(acc, expr)
}

/// Free variables referenced in `expr` that are not bound by `bound`.
pub fn free_vars_expr(expr: &Expr, bound: &[String]) -> Vec<String> {
    let mut bound = bound.to_vec();
    let mut out = Vec::new();
    collect_free_vars(expr, &mut bound, &mut out);
    out
}

fn collect_free_vars(expr: &Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
    match expr {
        Expr::Var(n, _) => {
            if !bound.iter().any(|b| b == n) && !out.iter().any(|x| x == n) {
                out.push(n.clone());
            }
        }
        Expr::Let {
            name, value, body, .. } => {
            collect_free_vars(value, bound, out);
            bound.push(name.clone());
            collect_free_vars(body, bound, out);
            bound.pop();
        }
        Expr::Lambda { params, body, .. } => {
            let n = bound.len();
            for p in params {
                bound.push(p.clone());
            }
            collect_free_vars(body, bound, out);
            bound.truncate(n);
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => {
            collect_free_vars(value, bound, out);
        }
        Expr::Call { callee, args, .. } => {
            collect_free_vars(callee, bound, out);
            for a in args {
                collect_free_vars(a, bound, out);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_free_vars(left, bound, out);
            collect_free_vars(right, bound, out);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_free_vars(cond, bound, out);
            collect_free_vars(then_branch, bound, out);
            collect_free_vars(else_branch, bound, out);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            collect_free_vars(cond, bound, out);
            collect_free_vars(body, bound, out);
            if let Some(s) = step {
                collect_free_vars(s, bound, out);
            }
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                collect_free_vars(s, bound, out);
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                collect_free_vars(a, bound, out);
            }
        }
        Expr::Return { value, .. } => collect_free_vars(value, bound, out),
        Expr::Alt { scrutinee, alt, .. } => {
            collect_free_vars(scrutinee, bound, out);
            collect_free_vars(alt, bound, out);
        }
        Expr::With { base, fields, .. } => {
            collect_free_vars(base, bound, out);
            for (_, e) in fields {
                collect_free_vars(e, bound, out);
            }
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(_)
        | Expr::Break(_)
        | Expr::Continue(_) => {}
    }
}

/// Collect all free variable names in `expr` (no outer bindings).
pub fn all_free_vars(expr: &Expr) -> HashSet<String> {
    free_vars_expr(expr, &[]).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_syntax::Span;

    #[test]
    fn free_vars_respects_lambda_binders() {
        let e = Expr::Lambda {
            params: vec!["x".into()],
            param_ann: vec![],
            body: Box::new(Expr::Var("y".into(), Span::dummy())),
            span: Span::dummy(),
        };
        assert_eq!(free_vars_expr(&e, &[]), vec!["y".to_string()]);
        assert!(free_vars_expr(&e, &["y".into()]).is_empty());
    }

    #[test]
    fn fold_counts_nodes() {
        let e = Expr::Binary {
            op: lumia_syntax::BinOp::Add,
            left: Box::new(Expr::Int(1, Span::dummy())),
            right: Box::new(Expr::Int(2, Span::dummy())),
            span: Span::dummy(),
        };
        let n = fold(&e, 0u32, |acc, _| acc + 1);
        assert_eq!(n, 3);
    }
}

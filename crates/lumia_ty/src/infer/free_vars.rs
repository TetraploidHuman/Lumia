//! Free-variable names in HIR expressions (for spawn capture checks).

use lumia_hir::Expr;
use rustc_hash::FxHashSet as HashSet;

/// Collect names referenced free in `expr` (not bound by nested lets/lambdas).
pub(crate) fn free_var_names(expr: &Expr) -> HashSet<String> {
    let mut bound = HashSet::default();
    let mut free = HashSet::default();
    walk(expr, &mut bound, &mut free);
    free
}

fn walk(expr: &Expr, bound: &mut HashSet<String>, free: &mut HashSet<String>) {
    match expr {
        Expr::Var(name, _) => {
            if !bound.contains(name) {
                free.insert(name.clone());
            }
        }
        Expr::Let {
            name,
            value,
            body,
            ..
        } => {
            walk(value, bound, free);
            let inserted = bound.insert(name.clone());
            walk(body, bound, free);
            if inserted {
                bound.remove(name);
            }
        }
        Expr::Assign { name, value, .. } => {
            if !bound.contains(name) {
                free.insert(name.clone());
            }
            walk(value, bound, free);
        }
        Expr::Lambda { params, body, .. } => {
            let mut added = Vec::new();
            for p in params {
                if bound.insert(p.clone()) {
                    added.push(p.clone());
                }
            }
            walk(body, bound, free);
            for p in added {
                bound.remove(&p);
            }
        }
        Expr::Call { callee, args, .. } => {
            walk(callee, bound, free);
            for a in args {
                walk(a, bound, free);
            }
        }
        Expr::Binary { left, right, .. } => {
            walk(left, bound, free);
            walk(right, bound, free);
        }
        Expr::Unary { expr, .. } | Expr::Return { value: expr, .. } => {
            walk(expr, bound, free);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            walk(cond, bound, free);
            walk(then_branch, bound, free);
            walk(else_branch, bound, free);
        }
        Expr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            walk(cond, bound, free);
            walk(body, bound, free);
            if let Some(s) = step {
                walk(s, bound, free);
            }
        }
        Expr::Alt {
            scrutinee, alt, ..
        } => {
            walk(scrutinee, bound, free);
            walk(alt, bound, free);
        }
        Expr::With { base, fields, .. } => {
            walk(base, bound, free);
            for (_, e) in fields {
                walk(e, bound, free);
            }
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                walk(s, bound, free);
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                walk(a, bound, free);
            }
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Break(..)
        | Expr::Continue(..) => {}
    }
}

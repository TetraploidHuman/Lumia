use super::*;
use lumia_hir::{lower_module, Builtin, Expr, Item};
use lumia_syntax::parse_module;

fn contains_list_par_map(e: &Expr) -> bool {
    match e {
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => true,
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            args.iter().any(contains_list_par_map)
        }
        Expr::Let { value, body, .. } => {
            contains_list_par_map(value) || contains_list_par_map(body)
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => {
            contains_list_par_map(value)
        }
        Expr::Lambda { body, .. } => contains_list_par_map(body),
        Expr::Call { callee, args, .. } => {
            contains_list_par_map(callee) || args.iter().any(contains_list_par_map)
        }
        Expr::Binary { left, right, .. } => {
            contains_list_par_map(left) || contains_list_par_map(right)
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            contains_list_par_map(cond)
                || contains_list_par_map(then_branch)
                || contains_list_par_map(else_branch)
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            contains_list_par_map(cond)
                || contains_list_par_map(body)
                || step.as_ref().is_some_and(|s| contains_list_par_map(s))
        }
        Expr::Seq { stmts, .. } => stmts.iter().any(contains_list_par_map),
        _ => false,
    }
}

mod effects;
mod infer;
mod parallel;
mod products;
mod soundness;
mod traits;

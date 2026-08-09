use super::*;

pub(super) fn expr_uses_ident(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Ident(n, _) => n == name,
        Expr::Block { stmts, tail, .. } => {
            stmts.iter().any(|s| stmt_uses_ident(s, name))
                || tail.as_ref().is_some_and(|e| expr_uses_ident(e, name))
        }
        Expr::Lambda { body, .. } => expr_uses_ident(body, name),
        Expr::Call { callee, args, .. } => {
            expr_uses_ident(callee, name) || args.iter().any(|a| expr_uses_ident(a, name))
        }
        Expr::Binary { left, right, .. } | Expr::Pipeline { left, right, .. } => {
            expr_uses_ident(left, name) || expr_uses_ident(right, name)
        }
        Expr::Unary { expr, .. } | Expr::Field { base: expr, .. } => expr_uses_ident(expr, name),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_uses_ident(cond, name)
                || expr_uses_ident(then_branch, name)
                || else_branch
                    .as_ref()
                    .is_some_and(|e| expr_uses_ident(e, name))
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_uses_ident(scrutinee, name)
                || arms.iter().any(|a| {
                    expr_uses_ident(&a.body, name)
                        || a.guard.as_ref().is_some_and(|g| expr_uses_ident(g, name))
                })
        }
        Expr::MatchCond { arms, .. } => arms.iter().any(|a| {
            a.cond.as_ref().is_some_and(|c| expr_uses_ident(c, name))
                || expr_uses_ident(&a.body, name)
        }),
        Expr::Return { value, .. } => expr_uses_ident(value, name),
        Expr::Alt { scrutinee, alt, .. } => {
            expr_uses_ident(scrutinee, name) || expr_uses_ident(alt, name)
        }
        Expr::ListLit { elems, .. } => elems.iter().any(|e| expr_uses_ident(e, name)),
        Expr::StructLit { fields, .. } => fields.iter().any(|(_, e)| expr_uses_ident(e, name)),
        Expr::With { base, fields, .. } => {
            expr_uses_ident(base, name) || fields.iter().any(|(_, e)| expr_uses_ident(e, name))
        }
        Expr::TupleLit { elems, .. } => elems.iter().any(|e| expr_uses_ident(e, name)),
        Expr::Interp { parts, .. } => parts.iter().any(|p| match p {
            crate::InterpPart::Lit(_) => false,
            crate::InterpPart::Expr(e) => expr_uses_ident(e, name),
        }),
        Expr::Int(..) | Expr::Float(..) | Expr::Bool(..) | Expr::String(..) | Expr::Char(..) => {
            false
        }
    }
}

pub(super) fn stmt_uses_ident(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Val { expr, .. } | Stmt::Var { expr, .. } | Stmt::Assign { expr, .. } => {
            expr_uses_ident(expr, name)
        }
        Stmt::Expr(e) => expr_uses_ident(e, name),
        Stmt::ForIn { iter, body, .. } => {
            expr_uses_ident(iter, name) || expr_uses_ident(body, name)
        }
        Stmt::ForCond { cond, body, .. } => {
            expr_uses_ident(cond, name) || expr_uses_ident(body, name)
        }
        Stmt::Break(_) | Stmt::Continue(_) => false,
    }
}

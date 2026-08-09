//! Rewrite `import … as` aliases into builtin / ident names.

use super::item_file_id;
use lumia_syntax::{Item, Module};
use rustc_hash::FxHashMap as HashMap;

pub(super) fn rewrite_builtin_alias_idents(
    m: &mut Module,
    aliases: &HashMap<String, String>,
    entry_file: u32,
) {
    if aliases.is_empty() {
        return;
    }
    for it in &mut m.items {
        // Only rewrite code that originated in the entry file.
        let file = item_file_id(it);
        if file != entry_file {
            continue;
        }
        match it {
            Item::Val(v) => rewrite_expr_aliases(&mut v.body, aliases),
            Item::Type(_) | Item::Foreign(_) | Item::Trait(_) | Item::Instance(_) => {}
        }
    }
}

pub(super) fn rewrite_expr_aliases(e: &mut lumia_syntax::Expr, aliases: &HashMap<String, String>) {
    use lumia_syntax::Expr::*;
    match e {
        Ident(name, _) => {
            if let Some(canon) = aliases.get(name) {
                *name = canon.clone();
            }
        }
        Interp { parts, .. } => {
            for p in parts {
                if let lumia_syntax::InterpPart::Expr(ex) = p {
                    rewrite_expr_aliases(ex, aliases);
                }
            }
        }
        Block { stmts, tail, .. } => {
            for s in stmts {
                rewrite_stmt_aliases(s, aliases);
            }
            if let Some(t) = tail {
                rewrite_expr_aliases(t, aliases);
            }
        }
        Lambda { body, .. } => rewrite_expr_aliases(body, aliases),
        Call { callee, args, .. } => {
            rewrite_expr_aliases(callee, aliases);
            for a in args {
                rewrite_expr_aliases(a, aliases);
            }
        }
        Binary { left, right, .. } | Pipeline { left, right, .. } => {
            rewrite_expr_aliases(left, aliases);
            rewrite_expr_aliases(right, aliases);
        }
        Unary { expr, .. } => rewrite_expr_aliases(expr, aliases),
        If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr_aliases(cond, aliases);
            rewrite_expr_aliases(then_branch, aliases);
            if let Some(e) = else_branch {
                rewrite_expr_aliases(e, aliases);
            }
        }
        Match {
            scrutinee, arms, ..
        } => {
            rewrite_expr_aliases(scrutinee, aliases);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    rewrite_expr_aliases(g, aliases);
                }
                rewrite_expr_aliases(&mut a.body, aliases);
            }
        }
        MatchCond { arms, .. } => {
            for a in arms {
                if let Some(c) = &mut a.cond {
                    rewrite_expr_aliases(c, aliases);
                }
                rewrite_expr_aliases(&mut a.body, aliases);
            }
        }
        Return { value, .. } => rewrite_expr_aliases(value, aliases),
        Alt { scrutinee, alt, .. } => {
            rewrite_expr_aliases(scrutinee, aliases);
            rewrite_expr_aliases(alt, aliases);
        }
        Field { base, .. } => rewrite_expr_aliases(base, aliases),
        ListLit { elems, .. } | TupleLit { elems, .. } => {
            for el in elems {
                rewrite_expr_aliases(el, aliases);
            }
        }
        StructLit { fields, .. } => {
            for (_, ex) in fields {
                rewrite_expr_aliases(ex, aliases);
            }
        }
        With { base, fields, .. } => {
            rewrite_expr_aliases(base, aliases);
            for (_, ex) in fields {
                rewrite_expr_aliases(ex, aliases);
            }
        }
        Int(..) | Float(..) | Bool(..) | String(..) | Char(..) => {}
    }
}

pub(super) fn rewrite_stmt_aliases(s: &mut lumia_syntax::Stmt, aliases: &HashMap<String, String>) {
    use lumia_syntax::Stmt::*;
    match s {
        Val { expr, .. } | Var { expr, .. } | Assign { expr, .. } => {
            rewrite_expr_aliases(expr, aliases)
        }
        Expr(expr) => rewrite_expr_aliases(expr, aliases),
        ForIn { iter, body, .. } => {
            rewrite_expr_aliases(iter, aliases);
            rewrite_expr_aliases(body, aliases);
        }
        ForCond { cond, body, .. } => {
            rewrite_expr_aliases(cond, aliases);
            rewrite_expr_aliases(body, aliases);
        }
        Break(_) | Continue(_) => {}
    }
}

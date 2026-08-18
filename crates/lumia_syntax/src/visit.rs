//! Shared mutable walks over the syntax AST (spans, pretty, recovery).

use crate::span::Span;
use crate::{Expr, InterpPart, Item, MatchArm, MatchCondArm, Module, Pattern, Stmt, ValItem};

/// Apply `f` to every span in `m` (module, imports, items).
pub fn map_module_spans(m: &mut Module, f: &mut dyn FnMut(&mut Span)) {
    f(&mut m.span);
    for imp in &mut m.imports {
        f(&mut imp.span);
    }
    for it in &mut m.items {
        map_item_spans(it, f);
    }
}

/// Apply `f` to every span under `it`.
pub fn map_item_spans(it: &mut Item, f: &mut dyn FnMut(&mut Span)) {
    match it {
        Item::Val(v) => map_val_spans(v, f),
        Item::Type(t) => f(&mut t.span),
        Item::Foreign(foreign) => f(&mut foreign.span),
        Item::Trait(t) => {
            f(&mut t.span);
            for m in &mut t.methods {
                map_val_spans(m, f);
            }
        }
        Item::Instance(i) => {
            f(&mut i.span);
            for m in &mut i.methods {
                map_val_spans(m, f);
            }
        }
    }
}

fn map_val_spans(v: &mut ValItem, f: &mut dyn FnMut(&mut Span)) {
    f(&mut v.span);
    map_expr_spans(&mut v.body, f);
}

/// Apply `f` to every span under `e`.
pub fn map_expr_spans(e: &mut Expr, f: &mut dyn FnMut(&mut Span)) {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Bool(_, s)
        | Expr::String(_, s)
        | Expr::Char(_, s)
        | Expr::Ident(_, s) => f(s),
        Expr::Interp { parts, span } => {
            f(span);
            for p in parts {
                if let InterpPart::Expr(ex) = p {
                    map_expr_spans(ex, f);
                }
            }
        }
        Expr::Block { stmts, tail, span } => {
            f(span);
            for s in stmts {
                map_stmt_spans(s, f);
            }
            if let Some(t) = tail {
                map_expr_spans(t, f);
            }
        }
        Expr::Lambda { body, span, .. } => {
            f(span);
            map_expr_spans(body, f);
        }
        Expr::Call { callee, args, span } => {
            f(span);
            map_expr_spans(callee, f);
            for a in args {
                map_expr_spans(a, f);
            }
        }
        Expr::Binary {
            left, right, span, ..
        } => {
            f(span);
            map_expr_spans(left, f);
            map_expr_spans(right, f);
        }
        Expr::Unary { expr, span, .. } => {
            f(span);
            map_expr_spans(expr, f);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span,
        } => {
            f(span);
            map_expr_spans(cond, f);
            map_expr_spans(then_branch, f);
            if let Some(e) = else_branch {
                map_expr_spans(e, f);
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
        } => {
            f(span);
            map_expr_spans(scrutinee, f);
            for a in arms {
                map_arm_spans(a, f);
            }
        }
        Expr::MatchCond { arms, span } => {
            f(span);
            for a in arms {
                map_cond_arm_spans(a, f);
            }
        }
        Expr::Return { value, span } => {
            f(span);
            map_expr_spans(value, f);
        }
        Expr::Alt {
            scrutinee,
            alt,
            span,
        } => {
            f(span);
            map_expr_spans(scrutinee, f);
            map_expr_spans(alt, f);
        }
        Expr::Field { base, span, .. } => {
            f(span);
            map_expr_spans(base, f);
        }
        Expr::ListLit { elems, span } => {
            f(span);
            for el in elems {
                map_expr_spans(el, f);
            }
        }
        Expr::Pipeline { left, right, span } => {
            f(span);
            map_expr_spans(left, f);
            map_expr_spans(right, f);
        }
        Expr::StructLit { fields, span, .. } => {
            f(span);
            for (_, ex) in fields {
                map_expr_spans(ex, f);
            }
        }
        Expr::With { base, fields, span } => {
            f(span);
            map_expr_spans(base, f);
            for (_, ex) in fields {
                map_expr_spans(ex, f);
            }
        }
        Expr::TupleLit { elems, span } => {
            f(span);
            for el in elems {
                map_expr_spans(el, f);
            }
        }
        Expr::Scope {
            scheduler,
            body,
            span,
        } => {
            f(span);
            if let Some(s) = scheduler {
                map_expr_spans(s, f);
            }
            map_expr_spans(body, f);
        }
        Expr::Spawn { body, span } => {
            f(span);
            map_expr_spans(body, f);
        }
    }
}

fn map_stmt_spans(s: &mut Stmt, f: &mut dyn FnMut(&mut Span)) {
    match s {
        Stmt::Val { expr, span, .. } | Stmt::Var { expr, span, .. } => {
            f(span);
            map_expr_spans(expr, f);
        }
        Stmt::Assign { expr, span, .. } => {
            f(span);
            map_expr_spans(expr, f);
        }
        Stmt::Expr(e) => map_expr_spans(e, f),
        Stmt::ForIn {
            iter, body, span, ..
        } => {
            f(span);
            map_expr_spans(iter, f);
            map_expr_spans(body, f);
        }
        Stmt::ForCond {
            cond, body, span, ..
        } => {
            f(span);
            map_expr_spans(cond, f);
            map_expr_spans(body, f);
        }
        Stmt::Break(span) | Stmt::Continue(span) => f(span),
    }
}

fn map_arm_spans(a: &mut MatchArm, f: &mut dyn FnMut(&mut Span)) {
    f(&mut a.span);
    map_pat_spans(&mut a.pattern, f);
    if let Some(g) = &mut a.guard {
        map_expr_spans(g, f);
    }
    map_expr_spans(&mut a.body, f);
}

fn map_cond_arm_spans(a: &mut MatchCondArm, f: &mut dyn FnMut(&mut Span)) {
    f(&mut a.span);
    if let Some(c) = &mut a.cond {
        map_expr_spans(c, f);
    }
    map_expr_spans(&mut a.body, f);
}

fn map_pat_spans(p: &mut Pattern, f: &mut dyn FnMut(&mut Span)) {
    match p {
        Pattern::Wildcard(s)
        | Pattern::Int(_, s)
        | Pattern::Float(_, s)
        | Pattern::Bool(_, s)
        | Pattern::Char(_, s)
        | Pattern::String(_, s)
        | Pattern::Ident(_, s) => f(s),
        Pattern::Variant { args, span, .. } => {
            f(span);
            for a in args {
                map_pat_spans(a, f);
            }
        }
        Pattern::Tuple { elems, span } => {
            f(span);
            for e in elems {
                map_pat_spans(e, f);
            }
        }
        Pattern::List { elems, span, .. } => {
            f(span);
            for e in elems {
                map_pat_spans(e, f);
            }
        }
        Pattern::Struct { fields, span, .. } => {
            f(span);
            for (_, p) in fields {
                map_pat_spans(p, f);
            }
        }
        Pattern::Or(ps, span) => {
            f(span);
            for p in ps {
                map_pat_spans(p, f);
            }
        }
    }
}

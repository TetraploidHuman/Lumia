//! Stamp `Span.file` after parse (multi-file SourceMap).

use crate::{
    Expr, Import, InterpPart, Item, MatchArm, MatchCondArm, Module, Pattern, Stmt, TypeItem,
    ValItem,
};

pub fn stamp_module(m: &mut Module, file: u32) {
    m.span = m.span.with_file(file);
    for imp in &mut m.imports {
        stamp_import(imp, file);
    }
    for it in &mut m.items {
        stamp_item(it, file);
    }
}

fn stamp_import(imp: &mut Import, file: u32) {
    imp.span = imp.span.with_file(file);
}

fn stamp_item(it: &mut Item, file: u32) {
    match it {
        Item::Val(v) => stamp_val(v, file),
        Item::Type(t) => stamp_type(t, file),
        Item::Foreign(f) => {
            f.span = f.span.with_file(file);
        }
        Item::Trait(t) => {
            t.span = t.span.with_file(file);
        }
        Item::Instance(i) => {
            i.span = i.span.with_file(file);
        }
    }
}

fn stamp_val(v: &mut ValItem, file: u32) {
    v.span = v.span.with_file(file);
    stamp_expr(&mut v.body, file);
}

fn stamp_type(t: &mut TypeItem, file: u32) {
    t.span = t.span.with_file(file);
}

fn stamp_expr(e: &mut Expr, file: u32) {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Bool(_, s)
        | Expr::String(_, s)
        | Expr::Char(_, s)
        | Expr::Ident(_, s) => *s = s.with_file(file),
        Expr::Interp { parts, span } => {
            *span = span.with_file(file);
            for p in parts {
                if let InterpPart::Expr(ex) = p {
                    stamp_expr(ex, file);
                }
            }
        }
        Expr::Block { stmts, tail, span } => {
            *span = span.with_file(file);
            for s in stmts {
                stamp_stmt(s, file);
            }
            if let Some(t) = tail {
                stamp_expr(t, file);
            }
        }
        Expr::Lambda { body, span, .. } => {
            *span = span.with_file(file);
            stamp_expr(body, file);
        }
        Expr::Call { callee, args, span } => {
            *span = span.with_file(file);
            stamp_expr(callee, file);
            for a in args {
                stamp_expr(a, file);
            }
        }
        Expr::Binary {
            left, right, span, ..
        } => {
            *span = span.with_file(file);
            stamp_expr(left, file);
            stamp_expr(right, file);
        }
        Expr::Unary { expr, span, .. } => {
            *span = span.with_file(file);
            stamp_expr(expr, file);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span,
        } => {
            *span = span.with_file(file);
            stamp_expr(cond, file);
            stamp_expr(then_branch, file);
            if let Some(e) = else_branch {
                stamp_expr(e, file);
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            span,
        } => {
            *span = span.with_file(file);
            stamp_expr(scrutinee, file);
            for a in arms {
                stamp_arm(a, file);
            }
        }
        Expr::MatchCond { arms, span } => {
            *span = span.with_file(file);
            for a in arms {
                stamp_cond_arm(a, file);
            }
        }
        Expr::Field { base, span, .. } => {
            *span = span.with_file(file);
            stamp_expr(base, file);
        }
        Expr::ListLit { elems, span } => {
            *span = span.with_file(file);
            for el in elems {
                stamp_expr(el, file);
            }
        }
        Expr::Pipeline { left, right, span } => {
            *span = span.with_file(file);
            stamp_expr(left, file);
            stamp_expr(right, file);
        }
        Expr::StructLit { fields, span, .. } => {
            *span = span.with_file(file);
            for (_, ex) in fields {
                stamp_expr(ex, file);
            }
        }
        Expr::With { base, fields, span } => {
            *span = span.with_file(file);
            stamp_expr(base, file);
            for (_, ex) in fields {
                stamp_expr(ex, file);
            }
        }
        Expr::TupleLit { elems, span } => {
            *span = span.with_file(file);
            for el in elems {
                stamp_expr(el, file);
            }
        }
    }
}

fn stamp_stmt(s: &mut Stmt, file: u32) {
    match s {
        Stmt::Val { expr, span, .. } | Stmt::Var { expr, span, .. } => {
            *span = span.with_file(file);
            stamp_expr(expr, file);
        }
        Stmt::Assign { expr, span, .. } => {
            *span = span.with_file(file);
            stamp_expr(expr, file);
        }
        Stmt::Expr(e) => stamp_expr(e, file),
        Stmt::ForIn {
            iter, body, span, ..
        } => {
            *span = span.with_file(file);
            stamp_expr(iter, file);
            stamp_expr(body, file);
        }
        Stmt::ForCond {
            cond, body, span, ..
        } => {
            *span = span.with_file(file);
            stamp_expr(cond, file);
            stamp_expr(body, file);
        }
        Stmt::Break(span) | Stmt::Continue(span) => *span = span.with_file(file),
    }
}

fn stamp_arm(a: &mut MatchArm, file: u32) {
    a.span = a.span.with_file(file);
    stamp_pat(&mut a.pattern, file);
    if let Some(g) = &mut a.guard {
        stamp_expr(g, file);
    }
    stamp_expr(&mut a.body, file);
}

fn stamp_cond_arm(a: &mut MatchCondArm, file: u32) {
    a.span = a.span.with_file(file);
    if let Some(c) = &mut a.cond {
        stamp_expr(c, file);
    }
    stamp_expr(&mut a.body, file);
}

fn stamp_pat(p: &mut Pattern, file: u32) {
    match p {
        Pattern::Wildcard(s)
        | Pattern::Int(_, s)
        | Pattern::Float(_, s)
        | Pattern::Bool(_, s)
        | Pattern::Char(_, s)
        | Pattern::String(_, s)
        | Pattern::Ident(_, s) => {
            *s = s.with_file(file);
        }
        Pattern::Variant { args, span, .. } => {
            *span = span.with_file(file);
            for a in args {
                stamp_pat(a, file);
            }
        }
        Pattern::Tuple { elems, span } => {
            *span = span.with_file(file);
            for e in elems {
                stamp_pat(e, file);
            }
        }
        Pattern::List { elems, span, .. } => {
            *span = span.with_file(file);
            for e in elems {
                stamp_pat(e, file);
            }
        }
        Pattern::Struct { fields, span, .. } => {
            *span = span.with_file(file);
            for (_, p) in fields {
                stamp_pat(p, file);
            }
        }
        Pattern::Or(ps, span) => {
            *span = span.with_file(file);
            for p in ps {
                stamp_pat(p, file);
            }
        }
    }
}

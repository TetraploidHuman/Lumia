//! Expression / statement formatting.

use super::pat::format_pat;
use super::{escape_str, format_block_contents, pad};
use crate::{Expr, InterpPart, MatchArm, MatchCondArm, Stmt, UnOp};

pub(crate) fn format_expr(out: &mut String, e: &Expr, indent: usize) {
    match e {
        Expr::Int(n, _) => out.push_str(&n.to_string()),
        Expr::Float(n, _) => out.push_str(&n.to_string()),
        Expr::Bool(b, _) => out.push_str(if *b { "true" } else { "false" }),
        Expr::String(s, _) => {
            out.push('"');
            out.push_str(&escape_str(s));
            out.push('"');
        }
        Expr::Char(c, _) => {
            out.push('\'');
            match *c {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                other => out.push(other),
            }
            out.push('\'');
        }
        Expr::Ident(n, _) => out.push_str(n),
        Expr::Interp { parts, .. } => {
            out.push('"');
            for p in parts {
                match p {
                    InterpPart::Lit(s) => out.push_str(&escape_str(s)),
                    InterpPart::Expr(ex) => {
                        out.push_str("${");
                        format_expr(out, ex, indent);
                        out.push('}');
                    }
                }
            }
            out.push('"');
        }
        Expr::Block { stmts, tail, .. } => {
            out.push_str("{\n");
            for s in stmts {
                pad(out, indent + 1);
                format_stmt(out, s, indent + 1);
                out.push('\n');
            }
            if let Some(t) = tail {
                pad(out, indent + 1);
                format_expr(out, t, indent + 1);
                out.push('\n');
            }
            pad(out, indent);
            out.push('}');
        }
        Expr::Lambda {
            params,
            param_tys,
            bare_it,
            body,
            ..
        } => {
            let print_params = !params.is_empty() && !*bare_it;
            if print_params {
                out.push_str("{ ");
                for (i, n) in params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(n);
                    if let Some(Some(t)) = param_tys.get(i) {
                        out.push_str(": ");
                        out.push_str(t);
                    }
                }
                out.push_str(" -> ");
            } else {
                out.push('{');
            }
            match body.as_ref() {
                Expr::Block { .. } => {
                    out.push('\n');
                    format_block_contents(out, body, indent + 1);
                    out.push('\n');
                    pad(out, indent);
                    out.push('}');
                }
                _ => {
                    out.push(' ');
                    format_expr(out, body, indent);
                    out.push_str(" }");
                }
            }
        }
        Expr::Call { callee, args, .. } => {
            // Surface sugar that parser desugars to calls (DESIGN §4.11):
            // `a..b` / `a..<b` / `a to b` — print the written form for fmt/IDE.
            if let (Expr::Ident(name, _), [left, right]) = (callee.as_ref(), args.as_slice()) {
                let op = match name.as_str() {
                    "rangeInclusive" => Some(".."),
                    "range" => Some("..<"),
                    "to" => Some(" to "),
                    _ => None,
                };
                if let Some(op) = op {
                    format_expr(out, left, indent);
                    out.push_str(op);
                    format_expr(out, right, indent);
                    return;
                }
            }
            // Trailing closure: `f(a) { … }` / `f { … }` (last arg is Lambda/Block).
            let trailing = matches!(
                args.last(),
                Some(Expr::Lambda { .. } | Expr::Block { .. })
            );
            format_expr(out, callee, indent);
            let prefix = if trailing {
                &args[..args.len() - 1]
            } else {
                args.as_slice()
            };
            if !trailing || !prefix.is_empty() {
                out.push('(');
                for (i, a) in prefix.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_expr(out, a, indent);
                }
                out.push(')');
            }
            if trailing {
                out.push(' ');
                format_expr(out, args.last().unwrap(), indent);
            }
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            format_expr(out, left, indent);
            out.push(' ');
            out.push_str(&op.to_string());
            out.push(' ');
            format_expr(out, right, indent);
        }
        Expr::Unary { op, expr, .. } => {
            match op {
                UnOp::Neg => out.push('-'),
                // DESIGN: keyword `not`, never `!`.
                UnOp::Not => out.push_str("not "),
            }
            format_expr(out, expr, indent);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            out.push_str("if ");
            format_expr(out, cond, indent);
            out.push(' ');
            format_expr(out, then_branch, indent);
            if let Some(e) = else_branch {
                out.push_str(" else ");
                format_expr(out, e, indent);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            format_expr(out, scrutinee, indent);
            out.push_str(" match {\n");
            for a in arms {
                pad(out, indent + 1);
                format_arm(out, a, indent + 1);
                out.push('\n');
            }
            pad(out, indent);
            out.push('}');
        }
        Expr::MatchCond { arms, .. } => {
            out.push_str("match {\n");
            for a in arms {
                pad(out, indent + 1);
                format_cond_arm(out, a, indent + 1);
                out.push('\n');
            }
            pad(out, indent);
            out.push('}');
        }
        Expr::Return { value, .. } => {
            out.push_str("return ");
            format_expr(out, value, indent);
        }
        Expr::Alt { scrutinee, alt, .. } => {
            format_expr(out, scrutinee, indent);
            out.push_str(" alt ");
            format_expr(out, alt, indent);
        }
        Expr::Field { base, field, .. } => {
            format_expr(out, base, indent);
            out.push('.');
            out.push_str(field);
        }
        Expr::ListLit { elems, .. } => {
            out.push('[');
            for (i, el) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(out, el, indent);
            }
            out.push(']');
        }
        Expr::TupleLit { elems, .. } => {
            out.push('(');
            for (i, el) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(out, el, indent);
            }
            out.push(')');
        }
        Expr::Pipeline { left, right, .. } => {
            format_expr(out, left, indent);
            out.push_str(" >> ");
            format_expr(out, right, indent);
        }
        Expr::StructLit { name, fields, .. } => {
            out.push_str(name);
            out.push_str(" { ");
            for (i, (f, ex)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(f);
                out.push_str(" = ");
                format_expr(out, ex, indent);
            }
            out.push_str(" }");
        }
        Expr::With { base, fields, .. } => {
            format_expr(out, base, indent);
            out.push_str(" with { ");
            for (i, (f, ex)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(f);
                out.push_str(" = ");
                format_expr(out, ex, indent);
            }
            out.push_str(" }");
        }
        Expr::Scope {
            scheduler, body, ..
        } => {
            out.push_str("scope");
            if let Some(s) = scheduler {
                out.push('(');
                format_expr(out, s, indent);
                out.push(')');
            }
            out.push(' ');
            format_expr(out, body, indent);
        }
        Expr::Spawn { body, .. } => {
            out.push_str("spawn ");
            format_expr(out, body, indent);
        }
    }
}

pub(crate) fn format_stmt(out: &mut String, s: &Stmt, indent: usize) {
    match s {
        Stmt::Val { pat, expr, .. } => {
            out.push_str("val ");
            format_pat(out, pat);
            out.push_str(" = ");
            format_expr(out, expr, indent);
        }
        Stmt::Var { name, expr, .. } => {
            out.push_str("var ");
            out.push_str(name);
            out.push_str(" = ");
            format_expr(out, expr, indent);
        }
        Stmt::Assign { name, expr, .. } => {
            out.push_str(name);
            out.push_str(" = ");
            format_expr(out, expr, indent);
        }
        Stmt::Expr(e) => format_expr(out, e, indent),
        Stmt::ForIn {
            binding,
            iter,
            body,
            ..
        } => {
            out.push_str("for ");
            match binding {
                crate::ForBinding::Name(n) => out.push_str(n),
                crate::ForBinding::Pair(a, b) => {
                    out.push('(');
                    out.push_str(a);
                    out.push_str(", ");
                    out.push_str(b);
                    out.push(')');
                }
            }
            out.push_str(" in ");
            format_expr(out, iter, indent);
            out.push(' ');
            format_expr(out, body, indent);
        }
        Stmt::ForCond { cond, body, .. } => {
            out.push_str("for ");
            format_expr(out, cond, indent);
            out.push(' ');
            format_expr(out, body, indent);
        }
        Stmt::Break(_) => out.push_str("break"),
        Stmt::Continue(_) => out.push_str("continue"),
    }
}

pub(crate) fn format_arm(out: &mut String, a: &MatchArm, indent: usize) {
    format_pat(out, &a.pattern);
    if let Some(g) = &a.guard {
        out.push_str(" if ");
        format_expr(out, g, indent);
    }
    out.push_str(" -> ");
    format_expr(out, &a.body, indent);
}

pub(crate) fn format_cond_arm(out: &mut String, a: &MatchCondArm, indent: usize) {
    match &a.cond {
        None => out.push('_'),
        Some(c) => format_expr(out, c, indent),
    }
    out.push_str(" -> ");
    format_expr(out, &a.body, indent);
}

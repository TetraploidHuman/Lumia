//! Basic pretty-printer for `lumia fmt` (DESIGN: 4-space indent).

use crate::{
    Expr, ImportNames, InterpPart, Item, MatchArm, MatchCondArm, Module, Pattern, Stmt,
    TypeKind, UnOp, ValItem, VariantFields,
};

pub fn format_module_src(m: &Module) -> String {
    let mut out = String::new();
    out.push_str("module ");
    out.push_str(&m.name);
    out.push('\n');
    if !m.imports.is_empty() || !m.items.is_empty() {
        out.push('\n');
    }
    for imp in &m.imports {
        out.push_str("import ");
        match &imp.names {
            ImportNames::All => {
                out.push_str(&imp.path.join("."));
                out.push_str(".*");
            }
            ImportNames::Single(n) if imp.path.is_empty() => {
                out.push_str(n);
            }
            ImportNames::Single(n) => {
                out.push_str(&imp.path.join("."));
                out.push('.');
                out.push_str(n);
            }
            ImportNames::Selective(ns) => {
                out.push_str(&imp.path.join("."));
                out.push_str(".{");
                out.push_str(&ns.join(", "));
                out.push('}');
            }
        }
        out.push('\n');
    }
    if !m.imports.is_empty() && !m.items.is_empty() {
        out.push('\n');
    }
    for (i, it) in m.items.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match it {
            Item::Val(v) => format_val(&mut out, v, 0),
            Item::Type(t) => {
                if t.is_priv {
                    out.push_str("priv ");
                }
                out.push_str("type ");
                out.push_str(&t.name);
                out.push_str(" =\n");
                match &t.kind {
                    TypeKind::Product(fs) => {
                        for f in fs {
                            out.push_str("    val ");
                            out.push_str(f);
                            out.push('\n');
                        }
                    }
                    TypeKind::Sum(vs) => {
                        for v in vs {
                            out.push_str("    ");
                            out.push_str(&v.name);
                            match &v.fields {
                                VariantFields::Unit => {}
                                VariantFields::Positional(n) => {
                                    out.push('(');
                                    out.push_str(&vec!["_"; *n].join(", "));
                                    out.push(')');
                                }
                                VariantFields::Named(fs) => {
                                    out.push_str(" { ");
                                    out.push_str(&fs.join(", "));
                                    out.push_str(" }");
                                }
                            }
                            out.push('\n');
                        }
                    }
                }
            }
            Item::Foreign(f) => {
                out.push_str("foreign \"");
                out.push_str(&f.abi);
                out.push_str("\" ");
                if f.is_pure {
                    out.push_str("pure ");
                }
                out.push_str("fn ");
                out.push_str(&f.name);
                out.push('(');
                for (i, (n, t)) in f.params.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(n);
                    out.push_str(": ");
                    out.push_str(t);
                }
                out.push_str(") -> ");
                out.push_str(&f.ret);
                out.push('\n');
            }
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn format_val(out: &mut String, v: &ValItem, indent: usize) {
    if v.is_priv {
        out.push_str("priv ");
    }
    out.push_str("val ");
    out.push_str(&v.name);
    if let Some(ps) = &v.params {
        out.push_str(" = { ");
        out.push_str(&ps.join(", "));
        out.push_str(" ->\n");
        format_expr(out, &v.body, indent + 1, true);
        out.push_str("\n}");
    } else {
        out.push_str(" = ");
        format_expr(out, &v.body, indent, false);
    }
    out.push('\n');
}

fn pad(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push_str("    ");
    }
}

fn format_expr(out: &mut String, e: &Expr, indent: usize, blockish: bool) {
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
            out.push(*c);
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
                        format_expr(out, ex, indent, false);
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
                format_expr(out, t, indent + 1, false);
                out.push('\n');
            }
            pad(out, indent);
            out.push('}');
        }
        Expr::Lambda { params, body, .. } => {
            out.push_str("{ ");
            if !params.is_empty() {
                out.push_str(&params.join(", "));
                out.push_str(" -> ");
            }
            format_expr(out, body, indent, false);
            out.push_str(" }");
        }
        Expr::Call { callee, args, .. } => {
            format_expr(out, callee, indent, false);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(out, a, indent, false);
            }
            out.push(')');
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            format_expr(out, left, indent, false);
            out.push(' ');
            out.push_str(&op.to_string());
            out.push(' ');
            format_expr(out, right, indent, false);
        }
        Expr::Unary { op, expr, .. } => {
            out.push(match op {
                UnOp::Neg => '-',
                UnOp::Not => '!',
            });
            format_expr(out, expr, indent, false);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            out.push_str("if ");
            format_expr(out, cond, indent, false);
            out.push(' ');
            format_expr(out, then_branch, indent, true);
            if let Some(e) = else_branch {
                out.push_str(" else ");
                format_expr(out, e, indent, true);
            }
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            format_expr(out, scrutinee, indent, false);
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
        Expr::Field { base, field, .. } => {
            format_expr(out, base, indent, false);
            out.push('.');
            out.push_str(field);
        }
        Expr::ListLit { elems, .. } => {
            out.push('[');
            for (i, el) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(out, el, indent, false);
            }
            out.push(']');
        }
        Expr::TupleLit { elems, .. } => {
            out.push('(');
            for (i, el) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(out, el, indent, false);
            }
            out.push(')');
        }
        Expr::Pipeline { left, right, .. } => {
            format_expr(out, left, indent, false);
            out.push_str(" >> ");
            format_expr(out, right, indent, false);
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
                format_expr(out, ex, indent, false);
            }
            out.push_str(" }");
        }
        Expr::With { base, fields, .. } => {
            format_expr(out, base, indent, false);
            out.push_str(" with { ");
            for (i, (f, ex)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(f);
                out.push_str(" = ");
                format_expr(out, ex, indent, false);
            }
            out.push_str(" }");
        }
    }
    let _ = blockish;
}

fn format_stmt(out: &mut String, s: &Stmt, indent: usize) {
    match s {
        Stmt::Val { name, expr, .. } => {
            out.push_str("val ");
            out.push_str(name);
            out.push_str(" = ");
            format_expr(out, expr, indent, false);
        }
        Stmt::Var { name, expr, .. } => {
            out.push_str("var ");
            out.push_str(name);
            out.push_str(" = ");
            format_expr(out, expr, indent, false);
        }
        Stmt::Assign { name, expr, .. } => {
            out.push_str(name);
            out.push_str(" = ");
            format_expr(out, expr, indent, false);
        }
        Stmt::Expr(e) => format_expr(out, e, indent, false),
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
            format_expr(out, iter, indent, false);
            out.push(' ');
            format_expr(out, body, indent, true);
        }
        Stmt::ForCond { cond, body, .. } => {
            out.push_str("for ");
            format_expr(out, cond, indent, false);
            out.push(' ');
            format_expr(out, body, indent, true);
        }
        Stmt::Break(_) => out.push_str("break"),
        Stmt::Continue(_) => out.push_str("continue"),
    }
}

fn format_arm(out: &mut String, a: &MatchArm, indent: usize) {
    format_pat(out, &a.pattern);
    if let Some(g) = &a.guard {
        out.push_str(" if ");
        format_expr(out, g, indent, false);
    }
    out.push_str(" -> ");
    format_expr(out, &a.body, indent, false);
}

fn format_cond_arm(out: &mut String, a: &MatchCondArm, indent: usize) {
    match &a.cond {
        None => out.push('_'),
        Some(c) => format_expr(out, c, indent, false),
    }
    out.push_str(" -> ");
    format_expr(out, &a.body, indent, false);
}

fn format_pat(out: &mut String, p: &Pattern) {
    match p {
        Pattern::Wildcard(_) => out.push('_'),
        Pattern::Int(n, _) => out.push_str(&n.to_string()),
        Pattern::Float(n, _) => out.push_str(&n.to_string()),
        Pattern::Bool(b, _) => out.push_str(if *b { "true" } else { "false" }),
        Pattern::Char(c, _) => {
            out.push('\'');
            match *c {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                other => out.push(other),
            }
            out.push('\'');
        }
        Pattern::String(s, _) => {
            out.push('"');
            out.push_str(&escape_str(s));
            out.push('"');
        }
        Pattern::Ident(n, _) => out.push_str(n),
        Pattern::Variant { name, args, .. } => {
            out.push_str(name);
            if !args.is_empty() {
                out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_pat(out, a);
                }
                out.push(')');
            }
        }
        Pattern::Tuple { elems, .. } => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_pat(out, e);
            }
            out.push(')');
        }
        Pattern::List { elems, rest, .. } => {
            out.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_pat(out, e);
            }
            if let Some(r) = rest {
                if !elems.is_empty() {
                    out.push_str(", ");
                }
                out.push_str("..");
                out.push_str(r);
            }
            out.push(']');
        }
        Pattern::Struct { name, fields, .. } => {
            out.push_str(name);
            out.push_str(" { ");
            for (i, (f, p)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(f);
                out.push_str(" = ");
                format_pat(out, p);
            }
            out.push_str(" }");
        }
        Pattern::Or(ps, _) => {
            for (i, p) in ps.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                format_pat(out, p);
            }
        }
    }
}

fn escape_str(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '$' => o.push_str("\\$"),
            c => o.push(c),
        }
    }
    o
}

//! Basic pretty-printer for `lumia fmt` (DESIGN: 4-space indent).

use crate::{
    Expr, ImportNames, ImportedName, InterpPart, Item, MatchArm, MatchCondArm, Module, Pattern,
    Stmt, TypeKind, UnOp, ValItem, VariantFields,
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
                format_imported_name(&mut out, n);
            }
            ImportNames::Single(n) => {
                out.push_str(&imp.path.join("."));
                out.push('.');
                format_imported_name(&mut out, n);
            }
            ImportNames::Selective(ns) => {
                out.push_str(&imp.path.join("."));
                out.push_str(".{");
                for (i, n) in ns.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_imported_name(&mut out, n);
                }
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
                out.push_str(" {\n");
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
                                    // AST keeps arity only; emit stable placeholder idents.
                                    out.push('(');
                                    for i in 0..*n {
                                        if i > 0 {
                                            out.push_str(", ");
                                        }
                                        out.push('v');
                                        out.push_str(&i.to_string());
                                    }
                                    out.push(')');
                                }
                                VariantFields::Named(fs) => {
                                    out.push_str(" {\n");
                                    for f in fs {
                                        out.push_str("        val ");
                                        out.push_str(f);
                                        out.push('\n');
                                    }
                                    out.push_str("    }");
                                }
                            }
                            out.push('\n');
                        }
                    }
                }
                out.push_str("}\n");
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
            Item::Trait(t) => {
                out.push_str("trait ");
                out.push_str(&t.name);
                if !t.requires.is_empty() {
                    out.push_str(" requires ");
                    out.push_str(&t.requires.join(", "));
                }
                out.push_str(" {\n");
                for m in &t.methods {
                    format_val(&mut out, m, 1);
                }
                out.push_str("}\n");
            }
            Item::Instance(i) => {
                out.push_str("instance ");
                out.push_str(&i.trait_name);
                out.push_str(" for ");
                out.push_str(&i.type_name);
                out.push_str(" {\n");
                for m in &i.methods {
                    format_val(&mut out, m, 1);
                }
                out.push_str("}\n");
            }
        }
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn format_imported_name(out: &mut String, n: &ImportedName) {
    out.push_str(&n.name);
    if let Some(a) = &n.alias {
        out.push_str(" as ");
        out.push_str(a);
    }
}

fn format_val(out: &mut String, v: &ValItem, indent: usize) {
    pad(out, indent);
    if v.is_priv {
        out.push_str("priv ");
    }
    out.push_str("val ");
    out.push_str(&v.name);
    if let Some(ps) = &v.params {
        // Lambda braces belong to the val sugar; unwrap a Block body so we do not
        // emit `val f = { x -> { ... } }`.
        out.push_str(" = { ");
        out.push_str(&ps.join(", "));
        out.push_str(" ->\n");
        format_block_contents(out, &v.body, indent + 1);
        out.push('\n');
        pad(out, indent);
        out.push('}');
    } else {
        out.push_str(" = ");
        format_expr(out, &v.body, indent);
    }
    out.push('\n');
}

/// Format `e` as the interior of an already-opened brace group (val params / similar).
fn format_block_contents(out: &mut String, e: &Expr, indent: usize) {
    match e {
        Expr::Block { stmts, tail, .. } => {
            for s in stmts {
                pad(out, indent);
                format_stmt(out, s, indent);
                out.push('\n');
            }
            if let Some(t) = tail {
                pad(out, indent);
                format_expr(out, t, indent);
            } else if !stmts.is_empty() {
                // Drop the trailing newline after the last statement; caller adds `\n}`.
                if out.ends_with('\n') {
                    out.pop();
                }
            }
        }
        other => {
            pad(out, indent);
            format_expr(out, other, indent);
        }
    }
}

fn pad(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push_str("    ");
    }
}

fn format_expr(out: &mut String, e: &Expr, indent: usize) {
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
        Expr::Lambda { params, body, .. } => {
            out.push_str("{ ");
            if !params.is_empty() {
                out.push_str(&params.join(", "));
                out.push_str(" -> ");
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
                    format_expr(out, body, indent);
                    out.push_str(" }");
                }
            }
        }
        Expr::Call { callee, args, .. } => {
            format_expr(out, callee, indent);
            out.push('(');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_expr(out, a, indent);
            }
            out.push(')');
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
    }
}

fn format_stmt(out: &mut String, s: &Stmt, indent: usize) {
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

fn format_arm(out: &mut String, a: &MatchArm, indent: usize) {
    format_pat(out, &a.pattern);
    if let Some(g) = &a.guard {
        out.push_str(" if ");
        format_expr(out, g, indent);
    }
    out.push_str(" -> ");
    format_expr(out, &a.body, indent);
}

fn format_cond_arm(out: &mut String, a: &MatchCondArm, indent: usize) {
    match &a.cond {
        None => out.push('_'),
        Some(c) => format_expr(out, c, indent),
    }
    out.push_str(" -> ");
    format_expr(out, &a.body, indent);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_module;

    /// Debug shape with span payloads erased so parse→fmt→parse can compare ASTs.
    fn shape(m: &Module) -> String {
        strip_spans(&format!("{m:?}"))
    }

    fn strip_spans(s: &str) -> String {
        let bytes = s.as_bytes();
        let mut out = String::new();
        let mut i = 0;
        while i < bytes.len() {
            if i + 4 <= bytes.len() && &s[i..i + 4] == "Span" {
                i += 4;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'{' {
                    let mut depth = 0;
                    while i < bytes.len() {
                        match bytes[i] {
                            b'{' => depth += 1,
                            b'}' => {
                                depth -= 1;
                                i += 1;
                                if depth == 0 {
                                    break;
                                }
                                continue;
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    out.push_str("Span");
                    continue;
                }
                out.push_str("Span");
                continue;
            }
            out.push(bytes[i] as char);
            i += 1;
        }
        out
    }

    fn roundtrip(src: &str) {
        roundtrip_opts(src, true);
    }

    /// `preserve_shape`: when false, only require parse + idempotent fmt (lossy nodes OK).
    fn roundtrip_opts(src: &str, preserve_shape: bool) {
        let m1 = parse_module(src).unwrap_or_else(|e| panic!("parse1: {e:?}\n{src}"));
        let formatted = format_module_src(&m1);
        let m2 = parse_module(&formatted)
            .unwrap_or_else(|e| panic!("parse2 after fmt: {e:?}\n---\n{formatted}"));
        if preserve_shape {
            assert_eq!(
                shape(&m1),
                shape(&m2),
                "fmt roundtrip shape mismatch\n--- formatted ---\n{formatted}"
            );
        }
        let formatted2 = format_module_src(&m2);
        assert_eq!(formatted, formatted2, "fmt not idempotent");
        let m3 = parse_module(&formatted2).expect("parse3");
        assert_eq!(shape(&m2), shape(&m3), "fmt unstable after second pass");
    }

    #[test]
    fn fmt_not_keyword_not_bang() {
        let src = r#"
module T
val main = {
    not true
}
"#;
        let m = parse_module(src).expect("parse");
        let out = format_module_src(&m);
        assert!(out.contains("not true"), "got:\n{out}");
        assert!(
            !out.contains("!true") && !out.contains("! true"),
            "got:\n{out}"
        );
        roundtrip(src);
    }

    #[test]
    fn fmt_import_as() {
        roundtrip(
            r#"
module T
import foo.{bar as baz, qux}
import math.add as plus
val main = 0
"#,
        );
    }

    #[test]
    fn fmt_hello_roundtrip() {
        roundtrip(
            r#"
module Hello
import std.io.{println}
val main = {
    println(42)
}
"#,
        );
    }

    #[test]
    fn fmt_val_params_unwraps_block() {
        let src = r#"
module T
val add = { a, b ->
    a + b
}
"#;
        let out = format_module_src(&parse_module(src).unwrap());
        assert!(!out.contains("->\n{\n"), "should not nest braces:\n{out}");
        roundtrip(src);
    }

    #[test]
    fn fmt_unary_neg_and_if() {
        roundtrip(
            r#"
module T
val main = {
    if not false {
        -1
    } else {
        0
    }
}
"#,
        );
    }

    #[test]
    fn fmt_match_and_list() {
        roundtrip(
            r#"
module T
val main = {
    val xs = [1, 2, 3]
    xs match {
        [] -> 0
        [h, ..t] -> h
    }
}
"#,
        );
    }

    #[test]
    fn fmt_type_product_and_sum() {
        roundtrip(
            r#"
module T
type Point {
    val x
    val y
}
val main = 0
"#,
        );
        // Positional field names are not stored in the AST — fmt uses v0,v1,…
        roundtrip_opts(
            r#"
module T
type Option {
    None
    Some(value)
}
val main = 0
"#,
            false,
        );
    }
}

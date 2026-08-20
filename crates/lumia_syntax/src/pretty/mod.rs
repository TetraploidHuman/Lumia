//! Basic pretty-printer for `lumia fmt` (DESIGN: 4-space indent).

mod expr;
mod pat;

use crate::{Expr, ImportNames, ImportedName, Item, Module, Sym, TypeKind, ValItem, VariantFields};
use expr::{format_expr, format_stmt};

fn syms_dot_join(path: &[Sym]) -> String {
    path.iter().map(Sym::as_str).collect::<Vec<_>>().join(".")
}

fn syms_comma_join(path: &[Sym]) -> String {
    path.iter().map(Sym::as_str).collect::<Vec<_>>().join(", ")
}

pub fn format_module_src(m: &Module) -> String {
    let mut out = String::new();
    out.push_str("module ");
    out.push_str(m.name.as_str());
    out.push('\n');
    if !m.imports.is_empty() || !m.items.is_empty() {
        out.push('\n');
    }
    for imp in &m.imports {
        out.push_str("import ");
        match &imp.names {
            ImportNames::All => {
                out.push_str(&syms_dot_join(&imp.path));
                out.push_str(".*");
            }
            ImportNames::Single(n) if imp.path.is_empty() => {
                format_imported_name(&mut out, n);
            }
            ImportNames::Single(n) => {
                out.push_str(&syms_dot_join(&imp.path));
                out.push('.');
                format_imported_name(&mut out, n);
            }
            ImportNames::Selective(ns) => {
                out.push_str(&syms_dot_join(&imp.path));
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
                                VariantFields::Positional(names) => {
                                    out.push('(');
                                    for (i, name) in names.iter().enumerate() {
                                        if i > 0 {
                                            out.push_str(", ");
                                        }
                                        out.push_str(name);
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
                    out.push_str(&syms_comma_join(&t.requires));
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

/// Whether `src` is already in `format_module_src` form.
///
/// Only normalizes a **missing final `\n`** (pretty-printer always emits one).
/// Does **not** strip trailing spaces — those must fail `fmt --check` and
/// produce an LSP edit (CLI used to `trim_end`, which falsely greened them).
pub fn format_matches_source(src: &str, formatted: &str) -> bool {
    fn with_final_newline(s: &str) -> std::borrow::Cow<'_, str> {
        if s.ends_with('\n') {
            std::borrow::Cow::Borrowed(s)
        } else {
            std::borrow::Cow::Owned(format!("{s}\n"))
        }
    }
    with_final_newline(src) == with_final_newline(formatted)
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
    if let Some(ty) = &v.ty {
        out.push_str(": ");
        out.push_str(ty);
    }
    if let Some(ps) = &v.params {
        // Lambda braces belong to the val sugar; unwrap a Block body so we do not
        // emit `val f = { x -> { ... } }`.
        out.push_str(" = { ");
        for (i, (n, ty)) in ps.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(n);
            if let Some(t) = ty {
                out.push_str(": ");
                out.push_str(t);
            }
        }
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
pub(crate) fn format_block_contents(out: &mut String, e: &Expr, indent: usize) {
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

pub(crate) use crate::escape::escape_str;

pub(crate) fn pad(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push_str("    ");
    }
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
    fn format_matches_source_ignores_only_missing_final_newline() {
        let src = "module T\n\nval x = 1\n";
        let m = parse_module(src).unwrap();
        let formatted = format_module_src(&m);
        assert!(format_matches_source(src, &formatted));
        assert!(format_matches_source(src.trim_end(), &formatted));
        // Trailing spaces on a line are a real drift (CLI check used to miss these).
        let dirty = "module T\n\nval x = 1  \n";
        assert!(!format_matches_source(dirty, &formatted));
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
    fn fmt_string_and_char_cr_roundtrip() {
        // Lexer accepts `\r`; pretty must emit it or fmt→parse loses CR.
        roundtrip(
            r#"
module T
val s = "a\rb"
val c = '\r'
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
    fn fmt_range_and_to_surface_sugar() {
        let src = r#"
module T
val main = {
    val a = 1..3
    val b = 1..<3
    val c = 1 to 3
    a.len() + b.len() + c.len()
}
"#;
        let m = parse_module(src).expect("parse");
        let formatted = format_module_src(&m);
        assert!(
            formatted.contains("1..3")
                && formatted.contains("1..<3")
                && formatted.contains("1 to 3"),
            "expected surface range/to sugar, got:\n{formatted}"
        );
        assert!(
            !formatted.contains("rangeInclusive") && !formatted.contains("range("),
            "should not print desugared call form:\n{formatted}"
        );
        roundtrip(src);
    }

    #[test]
    fn fmt_bare_it_surface_sugar() {
        let src = r#"
module T
val main = {
    val xs = [1, 2, 3]
    xs.map { it + 1 }
}
"#;
        let m = parse_module(src).expect("parse");
        let formatted = format_module_src(&m);
        assert!(
            formatted.contains("it + 1"),
            "expected bare it body, got:\n{formatted}"
        );
        assert!(
            !formatted.contains("it ->"),
            "should not invent explicit `it ->`:\n{formatted}"
        );
        assert!(
            formatted.contains("map {") && !formatted.contains("map({"),
            "expected trailing-closure form, got:\n{formatted}"
        );
        // Explicit `{ it -> … }` must still print the arrow.
        let explicit = r#"
module T
val main = {
    val xs = [1, 2, 3]
    xs.map { it -> it + 1 }
}
"#;
        let m2 = parse_module(explicit).expect("parse explicit");
        let out2 = format_module_src(&m2);
        assert!(
            out2.contains("it ->"),
            "explicit it param must keep arrow:\n{out2}"
        );
        roundtrip(src);
        roundtrip(explicit);
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
        // Positional binder names are preserved (`Some(value)` stays `value`).
        roundtrip(
            r#"
module T
type Option {
    None
    Some(value)
}
val main = 0
"#,
        );
    }
}

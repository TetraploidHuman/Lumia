use super::*;

#[test]
fn parse_for_in_ident_body_starts_with_assign() {
    // Regression: `for w in words { counts = ... }` must not parse
    // `words { counts = ... }` as a struct literal.
    let src = r#"
module T
val main = {
var counts = 0
val words = listOf(1)
for w in words {
    counts = w
}
counts
}
"#;
    parse_module(src).expect("parse for-in with assign body");
}

#[test]
fn parse_hello() {
    let src = r#"
module Hello
import std.io.{println}
val main = {
println(42)
}
"#;
    let m = parse_module(src).expect("parse");
    assert_eq!(m.name, "Hello");
    assert_eq!(m.items.len(), 1);
}

#[test]
fn parse_import_as_alias() {
    let m = parse_module(
        r#"
module T
import foo.{bar as baz, qux}
import math.add as plus
val main = 0
"#,
    )
    .expect("import as");
    assert_eq!(m.imports.len(), 2);
    match &m.imports[0].names {
        ImportNames::Selective(ns) => {
            assert_eq!(ns[0].name, "bar");
            assert_eq!(ns[0].alias.as_deref(), Some("baz"));
            assert_eq!(ns[1].name, "qux");
            assert!(ns[1].alias.is_none());
        }
        other => panic!("expected selective, got {other:?}"),
    }
    match &m.imports[1].names {
        ImportNames::Single(n) => {
            assert_eq!(n.name, "add");
            assert_eq!(n.alias.as_deref(), Some("plus"));
            assert_eq!(m.imports[1].path, vec![crate::Sym::from("math")]);
        }
        other => panic!("expected single, got {other:?}"),
    }
}

#[test]
fn parse_match_bare_expr_arms() {
    let src = r#"
module M
val f = { n ->
n match {
    0 -> 0
    1 -> 1
    x if x > 10 -> x - 1
    _ -> { n * 2 }
}
}
"#;
    let m = parse_module(src).expect("parse");
    assert_eq!(m.name, "M");
    let Item::Val(v) = &m.items[0] else {
        panic!("expected val");
    };
    let Expr::Lambda { body, .. } = &v.body else {
        panic!("expected lambda");
    };
    let Expr::Block { tail, .. } = body.as_ref() else {
        panic!("expected block body");
    };
    let Expr::Match { arms, .. } = tail.as_deref().expect("match tail") else {
        panic!("expected match");
    };
    assert_eq!(arms.len(), 4);
    assert!(!matches!(arms[0].body, Expr::Block { .. }));
    assert!(!matches!(arms[1].body, Expr::Block { .. }));
    assert!(!matches!(arms[2].body, Expr::Block { .. }));
    assert!(matches!(arms[3].body, Expr::Block { .. }));
}

#[test]
fn parse_match_tuple_arms_newline_paren() {
    // Bare arm body then newline `(…)` must start the next arm, not a call.
    let src = r#"
module M
val f = { t ->
t match {
    (1, x) -> x
    (2, y) -> y
    _ -> 0
}
}
"#;
    let m = parse_module(src).expect("parse");
    let Item::Val(v) = &m.items[0] else {
        panic!("expected val");
    };
    let Expr::Lambda { body, .. } = &v.body else {
        panic!("expected lambda");
    };
    let Expr::Block { tail, .. } = body.as_ref() else {
        panic!("expected block body");
    };
    let Expr::Match { arms, .. } = tail.as_deref().expect("match tail") else {
        panic!("expected match");
    };
    assert_eq!(arms.len(), 3);
}

#[test]
fn parse_map_set_literal_sugars() {
    let m = parse_module(
        r#"
module M
val main = {
val a = [:]
val b = [1 : 10, 2 : 20]
val c = #{}
val d = #{1, 2, 3}
a
}
"#,
    )
    .expect("parse map/set sugars");
    let Item::Val(v) = &m.items[0] else {
        panic!("expected val");
    };
    let Expr::Block { stmts, .. } = &v.body else {
        panic!("expected block");
    };
    assert_eq!(stmts.len(), 4);
    // [:] / [k:v] / #{} / #{…} desugar to mapOf/setOf calls
    for s in stmts {
        let Stmt::Val { expr, .. } = s else {
            panic!("expected val stmt");
        };
        assert!(
            matches!(expr, Expr::Call { .. }),
            "expected call sugar, got {expr:?}"
        );
    }
}

#[test]
fn parse_list_patterns_variants() {
    parse_module("module M\nval f = { xs -> xs match { [] -> 0 _ -> 1 }\n}\n").unwrap();
    parse_module("module M\nval f = { xs -> xs match { [h] -> h _ -> 0 }\n}\n").unwrap();
    parse_module("module M\nval f = { xs -> xs match { [..rest] -> 0 _ -> 1 }\n}\n").unwrap();
    parse_module("module M\nval f = { xs -> xs match { [h, ..rest] -> h _ -> 0 }\n}\n")
        .expect("h, ..rest");
}

#[test]
fn parse_val_tuple_destructure() {
    let m = parse_module("module M\nval main = {\n    val (a, b) = (1, 2)\n    a\n}\n")
        .expect("parse val (a,b)");
    let Item::Val(v) = &m.items[0] else {
        panic!("expected val");
    };
    let Expr::Block { stmts, .. } = &v.body else {
        panic!("expected block");
    };
    let Stmt::Val { pat, .. } = &stmts[0] else {
        panic!("expected val stmt");
    };
    assert!(matches!(pat, Pattern::Tuple { elems, .. } if elems.len() == 2));
}

#[test]
fn parse_constant_patterns() {
    parse_module(
        r#"
module M
val f = { b ->
b match {
    true -> 1
    false -> 0
}
}
"#,
    )
    .expect("bool patterns");
    parse_module(
        r#"
module M
val f = { c ->
c match {
    'a' -> 1
    _ -> 0
}
}
"#,
    )
    .expect("char pattern");
    parse_module(
        r#"
module M
val f = { s ->
s match {
    "hi" -> 1
    _ -> 0
}
}
"#,
    )
    .expect("string pattern");
    parse_module(
        r#"
module M
val f = { n ->
n match {
    -1 -> 1
    _ -> 0
}
}
"#,
    )
    .expect("negative int pattern");
}

#[test]
fn parse_string_interpolation() {
    let src = r#"
module M
val main = {
val name = "Lumia"
val n = 1
val s = "hi ${name} $n"
s
}
"#;
    let m = parse_module(src).expect("parse");
    assert_eq!(m.name, "M");
}

#[test]
fn parse_not_keyword() {
    let m = parse_module(
        r#"
module T
val main = {
    not false
}
"#,
    )
    .expect("parse not");
    assert_eq!(m.name, "T");
}

#[test]
fn recover_skips_bad_item_keeps_later() {
    let src = r#"
module Main
val add = { a, b -> a + b
val main = {
    1
}
"#;
    let out = crate::parse_module_recovering(src);
    assert!(!out.errors.is_empty(), "expected parse error on add");
    let names: Vec<_> = out
        .module
        .items
        .iter()
        .filter_map(|i| match i {
            crate::Item::Val(v) => Some(v.name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        names.contains(&"main"),
        "expected to recover `main`, got {names:?}, errors={:?}",
        out.errors
    );
}

#[test]
fn parse_kotlin_style_ranges() {
    let incl = parse_expr_str("1..5").expect("inclusive");
    match incl {
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident(n, _) => assert_eq!(n, "rangeInclusive"),
            other => panic!("expected Ident, got {other:?}"),
        },
        other => panic!("expected Call, got {other:?}"),
    }
    let excl = parse_expr_str("1..<5").expect("half-open");
    match excl {
        Expr::Call { callee, .. } => match callee.as_ref() {
            Expr::Ident(n, _) => assert_eq!(n, "range"),
            other => panic!("expected Ident, got {other:?}"),
        },
        other => panic!("expected Call, got {other:?}"),
    }
    let err = parse_expr_str("1..=5").expect_err("legacy ..=");
    assert!(
        err.message.contains("`..=`"),
        "expected helpful ..= error, got {}",
        err.message
    );
}

#[test]
fn interp_expr_spans_are_absolute() {
    let src = r#""hi${1+2}""#;
    let m = parse_module(&format!("module M\nval x = {src}\n")).expect("parse");
    let crate::Item::Val(v) = &m.items[0] else {
        panic!("expected val");
    };
    match &v.body {
        Expr::Interp { parts, .. } => {
            let e = match &parts[1] {
                crate::InterpPart::Expr(e) => e,
                other => panic!("expected expr part, got {other:?}"),
            };
            let Expr::Binary {
                left, right, span, ..
            } = e
            else {
                panic!("expected binary in interp, got {e:?}");
            };
            assert!(
                span.start.0 > 0,
                "interp binary span should be absolute, got {span:?}"
            );
            match (left.as_ref(), right.as_ref()) {
                (Expr::Int(1, ls), Expr::Int(2, rs)) => {
                    assert_eq!(rs.start.0, ls.start.0 + 2, "1+2 layout: {ls:?} {rs:?}");
                }
                other => panic!("expected 1+2, got {other:?}"),
            }
        }
        other => panic!("expected Interp, got {other:?}"),
    }
}

#[test]
fn interp_parse_error_uses_absolute_span() {
    let src = "module M\nval x = \"a${)}}\"\n";
    let err = parse_module(src).expect_err("bad interp");
    assert!(
        err.message.contains("interpolation") || err.message.contains("expected"),
        "{}",
        err.message
    );
    assert!(
        err.span.start.0 > 10,
        "expected absolute span inside string, got {:?}",
        err.span
    );
}

#[test]
fn nested_bare_it_lambda_does_not_capture_enclosing_block() {
    // `val main = { xs.map { it + 1 } }` must stay a 0-arg block, not `main(it)`.
    let src = r#"
module M
val main = {
    listOf(1).map { it + 1 }
}
"#;
    let m = parse_module(src).expect("parse");
    let crate::Item::Val(v) = &m.items[0] else {
        panic!("expected val");
    };
    match &v.body {
        Expr::Block { .. } => {}
        Expr::Lambda { params, .. } => panic!("main body became lambda with params {params:?}"),
        other => panic!("unexpected main body {other:?}"),
    }
    // Bare `{ it + 1 }` alone still becomes a 1-arg lambda.
    let lam = parse_expr_str("{ it + 1 }").expect("bare it");
    match lam {
        Expr::Lambda { params, .. } => assert_eq!(params, vec![crate::Sym::from("it")]),
        other => panic!("expected it-lambda, got {other:?}"),
    }
}

#[test]
fn duplicate_string_literals_share_sym() {
    use std::sync::Arc;
    let src = "module M\nval x = \"hello\" + \"hello\"\n";
    let m = parse_module(src).expect("parse");
    let crate::Item::Val(v) = &m.items[0] else {
        panic!("expected val");
    };
    let Expr::Binary { left, right, .. } = &v.body else {
        panic!("expected + on two strings, got {:?}", v.body);
    };
    let Expr::String(a, _) = left.as_ref() else {
        panic!("expected string lhs");
    };
    let Expr::String(b, _) = right.as_ref() else {
        panic!("expected string rhs");
    };
    assert_eq!(a, b);
    assert!(Arc::ptr_eq(a.arc(), b.arc()));
}

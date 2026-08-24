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
import lumi.io.{println}
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
            assert_eq!(m.imports[1].path, vec!["math".to_string()]);
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
val name = "Lumi"
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
fn parse_zero_arg_lambda_without_arrow() {
    let m = parse_module(
        r#"
module T
import lumi.io.{println}
val make = {
    { println(1) }
}
val side(x) = {
    println(x)
}
val block = {
    val x = 1
    x
}
val main = { make()() }
"#,
    )
    .expect("parse");
    let Item::Val(make) = &m.items[0] else {
        panic!("expected val make");
    };
    let Expr::Block { tail: Some(inner), .. } = &make.body else {
        panic!("expected block body for make, got {:?}", make.body);
    };
    assert!(
        matches!(inner.as_ref(), Expr::Lambda { .. }),
        "expected inner zero-arg lambda, got {:?}",
        inner
    );
    let Item::Val(side) = &m.items[1] else {
        panic!("expected val side");
    };
    assert!(
        matches!(side.body, Expr::Block { .. }),
        "function body must stay Block, got {:?}",
        side.body
    );
    let Item::Val(block) = &m.items[2] else {
        panic!("expected val block");
    };
    assert!(
        matches!(block.body, Expr::Block { .. }),
        "expected block with locals, got {:?}",
        block.body
    );
}


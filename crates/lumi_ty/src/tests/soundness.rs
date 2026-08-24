//! Soundness / capability regression probes (Eq, Ord, with, TuplePrefix, occurs).

use super::*;

fn infer_ok(src: &str) {
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("expected type-check ok");
}

fn infer_err(src: &str) -> String {
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir)
        .expect_err("expected type error")
        .message()
        .to_string()
}

#[test]
fn eq_rejects_function_literal() {
    let err = infer_err(
        r#"
module M
val main = { { x -> x } == { y -> y } }
"#,
    );
    assert!(
        err.contains("Eq") || err.contains("function"),
        "unexpected: {err}"
    );
}

#[test]
fn eq_rejects_function_ne() {
    let err = infer_err(
        r#"
module M
val main = { { x -> x } != { x -> x + 1 } }
"#,
    );
    assert!(
        err.contains("Eq") || err.contains("function"),
        "unexpected: {err}"
    );
}

#[test]
fn eq_poly_rejects_fun() {
    let err = infer_err(
        r#"
module M
val eq = { a, b -> a == b }
val main = { eq({ x -> 1 }, { x -> 2 }) }
"#,
    );
    assert!(
        err.contains("Eq") || err.contains("function") || err.contains("Fun"),
        "unexpected: {err}"
    );
}

#[test]
fn eq_allows_list_map_string() {
    infer_ok(
        r#"
module M
val eq = { a, b -> a == b }
val main = {
    eq(listOf(1), listOf(1))
    eq(mapOf(1, 2), mapOf(1, 2))
    eq("a", "b")
    0
}
"#,
    );
}

#[test]
fn ord_poly_allows_string_char() {
    infer_ok(
        r#"
module M
val cmp = { a, b -> a < b }
val main = {
    cmp("a", "b")
    cmp('a', 'b')
    0
}
"#,
    );
}

#[test]
fn ord_poly_rejects_set() {
    let err = infer_err(
        r#"
module M
val cmp = { a, b -> a < b }
val main = { cmp(setOf(1), setOf(2)) }
"#,
    );
    assert!(
        err.contains("Ord") || err.contains("Set"),
        "unexpected: {err}"
    );
}

#[test]
fn with_rejects_duplicate_field() {
    let err = infer_err(
        r#"
module M
type Point { val x val y }
val main = {
    val p = Point { x = 1, y = 2 }
    p with { x = 7, x = 9 }
}
"#,
    );
    assert!(
        err.contains("duplicate") && err.contains('x'),
        "unexpected: {err}"
    );
}

#[test]
fn with_open_ambiguous_shared_field_rejected() {
    let err = infer_err(
        r#"
module M
type Point { val x val y }
type Rect { val x val w }
val bump = { p -> p with { x = 1 } }
val main = {
    bump(Point { x = 0, y = 0 })
    0
}
"#,
    );
    assert!(
        err.contains("uniquely") || err.contains("open"),
        "unexpected: {err}"
    );
}

#[test]
fn with_open_unique_fields_ok() {
    infer_ok(
        r#"
module M
type Point { val x val y }
val bump = { p -> p with { x = 10 } }
val main = {
    val p2 = bump(Point { x = 3, y = 4 })
    p2.x + p2.y
}
"#,
    );
}

#[test]
fn with_concrete_rejects_cross_product_fields() {
    let err = infer_err(
        r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
    val p = Point { x = 1, y = 2 }
    p with { x = 7, w = 9 }
}
"#,
    );
    assert!(
        err.contains("unknown field") && err.contains('w'),
        "unexpected: {err}"
    );
}

#[test]
fn tuple_prefix_match_arms_reject_short() {
    let err = infer_err(
        r#"
module M
val f(c, t) = {
    c match {
        true -> t.0
        false -> t.0 + t.1
    }
}
val main = { f(true, (1,)) }
"#,
    );
    assert!(err.contains("tuple"), "unexpected: {err}");
}

#[test]
fn alt_open_receiver_rejected() {
    let err = infer_err(
        r#"
module M
type Option { Some(v) None }
val f = { x -> x alt 0 }
val main = { f(Some(1)) }
"#,
    );
    assert!(
        err.contains("alt") || err.contains("Option"),
        "unexpected: {err}"
    );
}

#[test]
fn num_poly_rejects_string_after_float() {
    let err = infer_err(
        r#"
module M
val dbl = { x -> x + x }
val main = {
    dbl(1.5)
    dbl("x")
}
"#,
    );
    assert!(
        err.contains("numeric") || err.contains("String"),
        "unexpected: {err}"
    );
}

#[test]
fn occurs_adt_and_tuple_unit() {
    use crate::infer::Infer;
    use crate::types::Type;
    let mut inf = Infer::new(crate::types::NameVisibility::default());
    let Type::Var(a) = inf.fresh() else { panic!() };
    let Type::Var(b) = inf.fresh() else { panic!() };
    inf.unify(
        Type::Var(a),
        Type::Adt {
            name: "Box".into(),
            params: vec![Type::Var(b)],
        },
    )
    .unwrap();
    assert!(inf
        .unify(
            Type::Var(b),
            Type::Adt {
                name: "Box".into(),
                params: vec![Type::Var(a)],
            },
        )
        .is_err());

    let mut inf = Infer::new(crate::types::NameVisibility::default());
    let Type::Var(a) = inf.fresh() else { panic!() };
    let Type::Var(b) = inf.fresh() else { panic!() };
    inf.unify(Type::Var(a), Type::Tuple(vec![Type::Var(b)]))
        .unwrap();
    assert!(inf
        .unify(Type::Var(b), Type::Tuple(vec![Type::Var(a)]))
        .is_err());
}

#[test]
fn open_with_unique_to_rect_rejects_point_arg() {
    let err = infer_err(
        r#"
module M
type Point { val x val y }
type Rect { val x val w }
val bump = { p -> p with { x = 1, w = 2 } }
val main = {
    bump(Point { x = 0, y = 0 })
    0
}
"#,
    );
    assert!(
        err.contains("mismatch") || err.contains("Point") || err.contains("Rect"),
        "unexpected: {err}"
    );
}

#[test]
fn eq_rejects_adt_and_list_holding_fun() {
    let err = infer_err(
        r#"
module M
type Box { Box(f) }
val main = {
    Box({ x -> x }) == Box({ x -> x })
}
"#,
    );
    assert!(
        err.contains("Eq") || err.contains("function"),
        "unexpected: {err}"
    );
    let err = infer_err(
        r#"
module M
val main = { listOf({ x -> x }) == listOf({ x -> x }) }
"#,
    );
    assert!(
        err.contains("Eq") || err.contains("function"),
        "unexpected: {err}"
    );
}

#[test]
fn eq_allows_tuple_of_ints() {
    infer_ok(
        r#"
module M
val main = { (1, 2) == (1, 2) }
"#,
    );
}

#[test]
fn ge_le_poly_ord_on_int() {
    infer_ok(
        r#"
module M
val le = { a, b -> a <= b }
val ge = { a, b -> a >= b }
val main = {
    le(1, 1)
    ge(2, 1)
}
"#,
    );
}

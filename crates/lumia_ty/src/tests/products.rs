use super::*;

#[test]
fn open_product_field_rejects_wrong_receiver() {
    let src = r#"
module M
type Point { val x val y }
val getx = { p -> p.x }
val main = { getx(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        infer_module(&hir).is_err(),
        "open field proj must not accept Int"
    );
}

#[test]
fn shared_product_field_resolves_from_receiver() {
    let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
    val p = Point { x = 1, y = 2 }
    val r = Rect { x = 3, w = 4 }
    println(p.x)
    println(r.x)
    val p2 = p with { x = 9 }
    println(p2.x)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("shared field names should resolve from receiver/`with` base");
}

#[test]
fn shared_product_field_open_receiver_still_errors() {
    let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val getx = { p -> p.x }
val main = { getx(Point { x = 1, y = 2 }) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        infer_module(&hir).is_err(),
        "open `{{ p -> p.x }}` with shared `x` must fail without a concrete receiver type"
    );
}

#[test]
fn open_tuple_proj_rejects_non_tuple() {
    let src = r#"
module M
val fst = { t -> t.0 }
val main = { fst(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("`.0` on Int must fail");
    assert!(
        err.message().contains("tuple") || err.message().contains("mismatch"),
        "got {}",
        err.message()
    );
}

#[test]
fn open_tuple_proj_accepts_longer_tuple() {
    let src = r#"
module M
val fst = { t -> t.0 }
val main = { fst((10, 20)) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("`.0` on pair must type-check");
}

#[test]
fn tuple_prefix_extend_rejects_short_tuple() {
    let src = r#"
module M
val use = { t ->
    val a = t.0
    val b = t.1
    a + b
}
val main = { use((10,)) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let r = infer_module(&hir);
    assert!(
        r.is_err(),
        "1-tuple must not satisfy body that projects `.1`; got Ok(use={:?})",
        r.as_ref()
            .ok()
            .and_then(|t| t.fun_types.get("use").cloned())
    );
}

#[test]
fn tuple_prefix_if_branch_rejects_short_tuple() {
    let src = r#"
module M
val f(c, t) = {
    if c {
        t.0
    } else {
        t.0 + t.1
    }
}
val main = { f(true, (1,)) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        infer_module(&hir).is_err(),
        "1-tuple must fail when other branch needs `.1`"
    );
}

#[test]
fn occurs_check_allows_equi_recursive_adt() {
    use crate::infer::Infer;
    use crate::types::Type;
    let mut inf = Infer::new(crate::types::NameVisibility::default());
    let Type::Var(a) = inf.fresh() else { panic!() };
    let Type::Var(b) = inf.fresh() else { panic!() };
    inf.unify(
        Type::Var(a),
        Type::Adt {
            name: "Option".into(),
            params: vec![Type::Var(b)],
        },
    )
    .expect("first bind");
    inf.unify(
        Type::Var(b),
        Type::Adt {
            name: "Option".into(),
            params: vec![Type::Var(a)],
        },
    )
    .expect("equi-recursive Adt cycle should be allowed");
}

#[test]
fn occurs_check_follows_subst_through_tuple() {
    use crate::infer::Infer;
    use crate::types::Type;
    let mut inf = Infer::new(crate::types::NameVisibility::default());
    let Type::Var(a) = inf.fresh() else { panic!() };
    let Type::Var(b) = inf.fresh() else { panic!() };
    inf.unify(Type::Var(a), Type::Tuple(vec![Type::Var(b)]))
        .expect("first");
    let r = inf.unify(Type::Var(b), Type::Tuple(vec![Type::Var(a)]));
    assert!(r.is_err(), "occurs through Tuple must fail; got Ok(())");
}

#[test]
fn occurs_check_must_follow_substitution_list() {
    use crate::infer::Infer;
    use crate::types::Type;
    let mut inf = Infer::new(crate::types::NameVisibility::default());
    let Type::Var(a) = inf.fresh() else { panic!() };
    let Type::Var(b) = inf.fresh() else { panic!() };
    inf.unify(Type::Var(a), Type::List(Box::new(Type::Var(b))))
        .expect("first bind");
    let r = inf.unify(Type::Var(b), Type::List(Box::new(Type::Var(a))));
    assert!(
        r.is_err(),
        "occurs must follow subst: β~List[α] with α=List[β] is infinite; got Ok(())"
    );
}

#[test]
fn ord_poly_rejects_list() {
    let src = r#"
module M
val cmp = { a, b -> a < b }
val main = { cmp(listOf(1), listOf(2)) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let r = infer_module(&hir);
    assert!(
        r.is_err(),
        "poly Ord via open Var must not accept List; got Ok({:?})",
        r.as_ref().ok().map(|t| t.fun_types.get("cmp").cloned())
    );
}

#[test]
fn ord_poly_rejects_fun() {
    let src = r#"
module M
val cmp = { a, b -> a < b }
val main = { cmp({ x -> x }, { x -> x }) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(infer_module(&hir).is_err(), "poly Ord must reject Fun");
}

#[test]
fn ord_poly_rejects_map() {
    let src = r#"
module M
val cmp = { a, b -> a < b }
val main = { cmp(mapOf(), mapOf()) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(infer_module(&hir).is_err(), "poly Ord must reject Map");
}

#[test]
fn with_rejects_foreign_product_fields() {
    let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
    val p = Point { x = 1, y = 2 }
    val q = p with { x = 7, w = 9 }
    q
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower defers with");
    assert!(
        infer_module(&hir).is_err(),
        "Point with {{ x, w }} must not become Rect; got Ok"
    );
}

use super::*;

#[test]
fn infer_hello() {
    let src = r#"
module Hello
import std.io.{println}
val main = {
    println(42)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).unwrap();
    check_effect_boundaries(&typed).unwrap();
    assert!(typed.main_effect.has_io());
}

#[test]
fn let_polymorphism_identity() {
    let src = r#"
module LetPoly
import std.io.{println}
val main = {
    val id = { x -> x }
    println(id(1))
    println(id("hi"))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("let-poly id");
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn map_of_empty() {
    let src = r#"
module M
val m = mapOf()
val main = {
    0
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).unwrap();
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn list_of_infers() {
    let src = r#"
module L
val xs = listOf(1, 2, 3)
val main = {
    0
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).unwrap();
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn if_and_add() {
    let src = r#"
module I
import std.io.{println}
val main = {
    val x = if true { 1 } else { 2 }
    println(x + 40)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).unwrap();
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn type_ascription_list_bracket_and_product() {
    let src = r#"
module AnnRich
import std.io.{println}
type Point { val x val y }
val head = { xs: List[Float] -> xs.get(0) }
val getx = { p: Point -> p.x }
val main = {
    println(head(listOf(1.5, 2.5)))
    println(getx(Point { x = 3.5, y = 0.0 }))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("rich ascriptions");
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn type_ascription_unknown_nominal_rejected() {
    let src = r#"
module Bad
val k: NoSuchType = 1
val main = { 0 }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("unknown type");
    assert!(
        err.message().contains("unknown type") || err.message().contains("NoSuchType"),
        "unexpected: {}",
        err.message()
    );
}

#[test]
fn type_ascription_val_and_lambda() {
    let src = r#"
module Ann
import std.io.{println}
val k: Int = 42
val add = { a: Int, b: Int -> a + b }
val main = {
    println(k)
    println(add(1, 2))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("ascription");
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn type_ascription_mismatch_rejected() {
    let src = r#"
module Bad
val k: Int = 1.5
val main = { 0 }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("Float vs Int");
    assert!(
        err.message().contains("mismatch") || err.message().contains("Float"),
        "unexpected: {}",
        err.message()
    );
}

#[test]
fn unify_mismatch_uses_display_type_not_debug() {
    let src = r#"
module M
val main = {
    val x = 1
    val y = "a"
    x + y
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("string+int");
    let msg = err.message();
    assert!(
        msg.contains("String") && msg.contains("Int"),
        "expected display_type names, got {msg}"
    );
    assert!(
        !msg.contains("Type::") && !msg.contains('?'),
        "must not dump Debug / ?N: {msg}"
    );
}

#[test]
fn sum_mixed_arity_shared_params() {
    let src = r#"
module ShapeMix
import std.io.{println}
type Shape {
    Circle(r)
    Rect(w, h)
}
val area = { s ->
    s match {
        Circle(r) -> r * r
        Rect(w, h) -> w * h
    }
}
val main = {
    println(area(Circle(3)))
    println(area(Rect(2, 5)))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("mixed-arity sum ADT");
    check_effect_boundaries(&typed).unwrap();
}


#[test]
fn recursive_expr_eval() {
    let src = r#"
module ExprRec
import std.io.{println}
type Expr { Lit(n) Add(l, r) }
val eval = { e ->
    e match {
        Lit(n) -> n
        Add(l, r) -> eval(l) + eval(r)
    }
}
val main = {
    println(eval(Lit(7)))
    println(eval(Add(Lit(2), Lit(3))))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("recursive Expr ADT (equi-recursive)");
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn recursive_nat_to_int() {
    let src = r#"
module NatRec
import std.io.{println}
type Nat { Z S(n) }
val toInt = { n ->
    n match {
        Z -> 0
        S(m) -> 1 + toInt(m)
    }
}
val main = {
    println(toInt(S(S(Z))))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("recursive Nat ADT");
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn match_int_arms() {
    let src = r#"
module MatchDemo
import std.io.{println}
val main = {
    val n = 1
    val s = n match {
        0 -> 10
        1 -> 20
        _ -> 30
    }
    println(s)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("infer");
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn println_does_not_freeze_var_to_int() {
    let src = r#"
module M
import std.io.{println}
val f = { x ->
    println(x)
    x
}
val main = {
    println(f(1.5))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("println must leave open Var unconstrained");
}

#[test]
fn builtin_arity_from_info_rejects_get() {
    use lumia_hir::{Builtin, Expr, Fun, Item, Module};
    use lumia_syntax::Span;
    use rustc_hash::{FxHashMap, FxHashSet};
    let span = Span::dummy();
    let hir = Module {
        name: "Bad".into(),
        items: vec![Item::Fun(Fun {
            name: "main".into(),
            params: vec![],
            param_ann: vec![],
            ret_ann: None,
            body: Expr::BuiltinCall {
                name: Builtin::ListGet,
                args: vec![Expr::Int(1, span)], // missing index
                span,
            },
            span,
            is_main: true,
            external: None,
            foreign_sig: None,
            foreign_pure: false,
            is_priv: false,
        })],
        adts: Vec::new(),
        products: Vec::new(),
        instances: FxHashSet::default(),
        trait_methods: FxHashMap::default(),
        method_traits: FxHashMap::default(),
    };
    let err = infer_module(&hir).expect_err("get arity");
    assert!(
        err.message().contains("get") && err.message().contains("argument"),
        "unexpected: {}",
        err.message()
    );
}

#[test]
fn typecheck_hir_runs_effects_and_parallel() {
    let src = r#"
module Ok
import std.io.{println}
val main = { println(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = typecheck_hir(
        &hir,
        NameVisibility::default(),
        &TypecheckOptions::default(),
    )
    .expect("typecheck");
    assert!(typed.main_effect.has_io());

    let bad = r#"
module Bad
import std.io.{println}
val xs = println(1)
val main = { 0 }
"#;
    let ast = parse_module(bad).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = typecheck_hir(
        &hir,
        NameVisibility::default(),
        &TypecheckOptions::default(),
    )
    .expect_err("top-level IO");
    assert!(
        err.message().to_lowercase().contains("effect")
            || err.message().to_lowercase().contains("io"),
        "unexpected: {}",
        err.message()
    );
}

#[test]
fn prelude_ctor_first_class_poly() {
    // First-class / alias use must not start from `List[Int]`/`Map[Int,Int]` stubs.
    let src = r#"
module CtorPoly
import std.io.{println}
val main = {
    val lo = listOf
    val so = setOf
    val mo = mapOf
    val xs = lo().concat(listOf(1.5))
    val s = so().insert(2.5)
    val m = mo().set(3.5, 4.5)
    println(xs.get(0))
    println(s.contains(2.5))
    println(m.get(3.5) alt 0.0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("poly ctor alias");
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn free_println_call_ok_without_import() {
    // Lower rewrites `println(…)` to BuiltinCall — no std.io import required.
    let src = r#"
module M
val main = { println(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("free println call");
}

#[test]
fn first_class_println_requires_import() {
    // Seeding println in Infer env false-greened `val f = println` on lone files.
    let src = r#"
module M
val f = println
val main = 0
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("first-class println needs import");
    assert!(
        err.message().contains("unbound") || err.message().contains("println"),
        "{}",
        err.message()
    );
}

#[test]
fn poly_id_ok_when_use_declared_first() {
    // Callee must generalize before callers even if it appears later in the file.
    let src = r#"
module M
import std.io.{println}
val use = { x -> id(x) + id(1) }
val id = { x -> x }
val main = { println(id(true)) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("id should stay polymorphic");
}

#[test]
fn break_outside_loop_is_error() {
    let src = r#"
module M
val main = {
    break
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("bare break");
    assert!(
        err.to_string().contains("break") && err.to_string().contains("loop"),
        "{err}"
    );
}

#[test]
fn continue_outside_loop_is_error() {
    let src = r#"
module M
val main = {
    continue
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("bare continue");
    assert!(
        err.to_string().contains("continue") && err.to_string().contains("loop"),
        "{err}"
    );
}

#[test]
fn break_inside_for_ok() {
    let src = r#"
module M
val main = {
    for i in 1..3 {
        break
    }
    0
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("break in for");
}

#[test]
fn break_inside_lambda_inside_loop_is_error() {
    let src = r#"
module M
val main = {
    for i in 1..3 {
        val f = { -> break }
        f()
    }
    0
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("break across lambda");
    assert!(
        err.to_string().contains("break") && err.to_string().contains("loop"),
        "{err}"
    );
}

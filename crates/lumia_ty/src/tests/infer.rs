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
        })],
        adts: Vec::new(),
        products: Vec::new(),
        instances: FxHashSet::default(),
        show_methods: FxHashMap::default(),
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

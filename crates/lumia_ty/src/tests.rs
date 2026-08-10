use super::*;
use lumia_hir::{lower_module, Builtin, Expr, Item};
use lumia_syntax::parse_module;

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
fn ord_rejects_list_compare() {
    let src = r#"
module BadOrd
val main = {
    listOf(1) < listOf(2)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("List is not Ord");
    assert!(
        err.message().contains("Ord") || err.message().contains("List"),
        "unexpected: {}",
        err.message()
    );
}

#[test]
fn ord_instance_allows_product_compare() {
    let src = r#"
module M
type Point { val x val y }
trait Eq { }
trait Ord requires Eq { }
instance Eq for Point { }
instance Ord for Point { }
val main = {
    Point { x = 1, y = 2 } < Point { x = 1, y = 3 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("Ord instance");
}

#[test]
fn num_instance_allows_adt_add() {
    let src = r#"
module M
type Vec2 { val x val y }
instance Num for Vec2 {
    val add = { self, other ->
        Vec2 { x = self.x + other.x, y = self.y + other.y }
    }
}
val main = {
    Vec2 { x = 1, y = 2 } + Vec2 { x = 3, y = 4 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("Num instance");
}

#[test]
fn adt_add_without_num_rejected() {
    let src = r#"
module M
type Vec2 { val x val y }
val main = {
    Vec2 { x = 1, y = 2 } + Vec2 { x = 3, y = 4 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(infer_module(&hir).is_err());
}

#[test]
fn unknown_trait_instance_rejected() {
    let src = r#"
module M
type Point { val x val y }
instance NotATrait for Point { }
val main = { 0 }
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).expect_err("unknown trait");
    assert!(
        err.message.contains("unknown trait") || err.message.contains("NotATrait"),
        "{err}"
    );
}

#[test]
fn auto_derive_eq_allows_ord_alone() {
    let src = r#"
module M
type Point { val x val y }
instance Ord for Point { }
val main = {
    Point { x = 1, y = 2 } < Point { x = 1, y = 3 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("auto Eq + Ord");
    infer_module(&hir).expect("Ord with auto Eq");
}

#[test]
fn arith_poly_rejects_string() {
    let src = r#"
module M
import std.io.{println}
val main = {
    val add1 = { x -> x + 1 }
    println(add1("hi"))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("String is not numeric");
    assert!(
        err.message().contains("numeric") || err.message().contains("String"),
        "unexpected: {}",
        err.message()
    );
}

#[test]
fn toplevel_num_poly_dbl() {
    let src = r#"
module M
import std.io.{println}
val dbl = { x -> x + x }
val main = {
    println(dbl(1))
    println(dbl(1.5))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("top-level Num poly");
}

#[test]
fn trait_poly_method_infers() {
    let src = r#"
module M
import std.io.{println}
type Point { val x val y }
type Box { val n }
trait ToInt { val toInt = { self -> 0 } }
instance ToInt for Point { val toInt = { self -> self.x } }
instance ToInt for Box { val toInt = { self -> self.n } }
val main = {
    val f = { x -> x.toInt() }
    println(f(Point { x = 7, y = 0 }))
    println(f(Box { n = 4 }))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    infer_module(&hir).expect("poly trait method");
}

#[test]
fn trait_poly_method_rejects_missing_instance() {
    let src = r#"
module M
import std.io.{println}
type Point { val x }
trait ToInt { val toInt = { self -> 0 } }
val main = {
    val f = { x -> x.toInt() }
    println(f(Point { x = 1 }))
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("missing ToInt instance");
    assert!(
        err.message().contains("ToInt") || err.message().contains("instance"),
        "unexpected: {}",
        err.message()
    );
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
fn pure_may_construct_io_thunk() {
    let src = r#"
module T
import std.io.{println}
val make() = { { -> println(1) } }
val main = { make()() }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).unwrap();
    check_effect_boundaries(&typed).unwrap();
    assert!(matches!(
        typed.fun_types.get("make"),
        Some(Type::Fun(_, _, Effect::Pure))
    ));
}

#[test]
fn calling_io_thunk_marks_caller_io() {
    let src = r#"
module T
import std.io.{println}
val apply(f) = f()
val compute() = {
    apply({ -> println(1) })
    0
}
val main = { compute() }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).unwrap();
    check_effect_boundaries(&typed).unwrap();
    assert!(
        matches!(
            typed.fun_types.get("compute"),
            Some(Type::Fun(_, _, Effect::Io))
        ),
        "got {:?}",
        typed.fun_types.get("compute")
    );
    assert!(
        matches!(
            typed.fun_types.get("apply"),
            Some(Type::Fun(_, _, Effect::Io))
        ),
        "got {:?}",
        typed.fun_types.get("apply")
    );
}

#[test]
fn reject_println_inside_pure_lambda_used_as_value() {
    let src = r#"
module Bad
import std.io.{println}
val compute() = {
    println(1)
    0
}
val main = {
    compute()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).unwrap();
    check_effect_boundaries(&typed).unwrap();
    assert!(matches!(
        typed.fun_types.get("compute"),
        Some(Type::Fun(_, _, Effect::Io))
    ));
}

#[test]
fn module_val_rejects_io() {
    let src = r#"
module Bad
import std.io.{println}
val xs = println(1)
val main = {
    0
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(infer_module(&hir).is_err());
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
fn hof_picks_up_callback_io() {
    let src = r#"
module Hof
import std.io.{println}
val apply(f, x) = f(x)
val boom(x) = {
    println(1)
    x
}
val main = {
    apply(boom, 42)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("infer");
    check_effect_boundaries(&typed).unwrap();
    assert!(matches!(
        typed.fun_types.get("apply"),
        Some(Type::Fun(_, _, Effect::Io))
    ));
    assert!(matches!(
        typed.fun_types.get("boom"),
        Some(Type::Fun(_, _, Effect::Io))
    ));
}

#[test]
fn hof_stays_pure_with_pure_callback() {
    let src = r#"
module HofPure
val apply(f, x) = f(x)
val id(x) = x
val main = {
    apply(id, 42)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("infer");
    check_effect_boundaries(&typed).unwrap();
    assert!(matches!(
        typed.fun_types.get("apply"),
        Some(Type::Fun(_, _, Effect::Pure))
    ));
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

fn contains_list_par_map(e: &Expr) -> bool {
    match e {
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => true,
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            args.iter().any(contains_list_par_map)
        }
        Expr::Let { value, body, .. } => {
            contains_list_par_map(value) || contains_list_par_map(body)
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => {
            contains_list_par_map(value)
        }
        Expr::Lambda { body, .. } => contains_list_par_map(body),
        Expr::Call { callee, args, .. } => {
            contains_list_par_map(callee) || args.iter().any(contains_list_par_map)
        }
        Expr::Binary { left, right, .. } => {
            contains_list_par_map(left) || contains_list_par_map(right)
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            contains_list_par_map(cond)
                || contains_list_par_map(then_branch)
                || contains_list_par_map(else_branch)
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            contains_list_par_map(cond)
                || contains_list_par_map(body)
                || step.as_ref().is_some_and(|s| contains_list_par_map(s))
        }
        Expr::Seq { stmts, .. } => stmts.iter().any(contains_list_par_map),
        _ => false,
    }
}

#[test]
fn parallel_map_io_demoted_to_sequential() {
    let src = r#"
module ParIo
import std.io.{println}
val boom(x) = {
    println(x + 0)
    x + 1
}
val main = {
    listOf(1, 2, 3).map(boom)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        contains_list_par_map(
            &hir.items
                .iter()
                .find_map(|i| match i {
                    Item::Fun(f) if f.is_main => Some(&f.body),
                    _ => None,
                })
                .unwrap()
        ),
        "FunRef-safe map should lower to ListParMap candidate"
    );
    let mut typed = infer_module(&hir).expect("IO map must type-check");
    finalize_auto_parallel(&mut typed, true);
    let main_body = typed
        .module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fun(f) if f.is_main => Some(&f.body),
            _ => None,
        })
        .unwrap();
    assert!(
        !contains_list_par_map(main_body),
        "impure map must be demoted after finalize_auto_parallel"
    );
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn parallel_map_pure_scalar_kept() {
    let src = r#"
module ParOk
val double(x) = x * 2
val main = {
    listOf(1, 2, 3).map(double)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let mut typed = infer_module(&hir).expect("infer");
    finalize_auto_parallel(&mut typed, true);
    let main_body = typed
        .module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fun(f) if f.is_main => Some(&f.body),
            _ => None,
        })
        .unwrap();
    assert!(
        contains_list_par_map(main_body),
        "pure scalar map should stay ListParMap"
    );
}

#[test]
fn parallel_map_toplevel_lambda_kept() {
    let src = r#"
module ParLam
val double(x) = x * 2
val main = {
    listOf(1, 2, 3).map({ x -> double(x) })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    assert!(
        contains_list_par_map(
            &hir.items
                .iter()
                .find_map(|i| match i {
                    Item::Fun(f) if f.is_main => Some(&f.body),
                    _ => None,
                })
                .unwrap()
        ),
        "lambda calling only top-level funs should lower to ListParMap"
    );
    let mut typed = infer_module(&hir).expect("infer");
    finalize_auto_parallel(&mut typed, true);
    let main_body = typed
        .module
        .items
        .iter()
        .find_map(|i| match i {
            Item::Fun(f) if f.is_main => Some(&f.body),
            _ => None,
        })
        .unwrap();
    assert!(
        contains_list_par_map(main_body),
        "toplevel-only lambda map should stay ListParMap"
    );
}

/// `if` arms joining Pure/Io function values must lub to Io on the caller.
#[test]
fn if_branches_io_vs_pure_fun_marks_caller_or_rejects() {
    let src = r#"
module Hole
import std.io.{println}
val id(x) = x
val boom(x) = {
    println(x + 0)
    x
}
val sneak(c, x) = {
    val f = if c { id } else { boom }
    f(x)
}
val main = {
    sneak(false, 1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("infer");
    assert!(
        matches!(
            typed.fun_types.get("sneak"),
            Some(Type::Fun(_, _, Effect::Io))
        ),
        "if-branch Fun lub must mark sneak Io; got {:?}",
        typed.fun_types.get("sneak")
    );
    check_effect_boundaries(&typed).unwrap();
}

/// Assigning an Io lambda into a `var` previously holding a Pure lambda widens ε.
#[test]
fn assign_io_fun_into_pure_var_marks_caller_or_rejects() {
    let src = r#"
module Hole
import std.io.{println}
val sneak(x) = {
    var f = { y -> y }
    f = { y ->
        println(y + 0)
        y
    }
    f(x)
}
val main = {
    sneak(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("infer");
    assert!(
        matches!(
            typed.fun_types.get("sneak"),
            Some(Type::Fun(_, _, Effect::Io))
        ),
        "assign Fun lub must mark sneak Io; got {:?}",
        typed.fun_types.get("sneak")
    );
    check_effect_boundaries(&typed).unwrap();
}

/// Two open callback effects in one body must not drop the second Var.
#[test]
fn hof_two_callbacks_union_preserves_io() {
    let src = r#"
module Both
import std.io.{println}
val both(f, g, x) = {
    f(x)
    g(x)
}
val id(x) = x
val boom(x) = {
    println(x + 0)
    x
}
val sneak(x) = both(id, boom, x)
val main = {
    sneak(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("infer");
    assert!(
        matches!(
            typed.fun_types.get("both"),
            Some(Type::Fun(_, _, Effect::Io))
        ),
        "both must be Io when either callback is Io; got {:?}",
        typed.fun_types.get("both")
    );
    assert!(
        matches!(
            typed.fun_types.get("sneak"),
            Some(Type::Fun(_, _, Effect::Io))
        ),
        "sneak must be Io; got {:?}",
        typed.fun_types.get("sneak")
    );
    check_effect_boundaries(&typed).unwrap();
}

#[test]
fn foreign_pure_requires_trust() {
    let src = r#"
module F
foreign "C" pure fn llabs(x: Int) -> Int
val main = { llabs(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let err = infer_module(&hir).expect_err("pure without trust");
    assert!(
        err.message().contains("trust-foreign-pure"),
        "got {}",
        err.message()
    );
}

#[test]
fn foreign_pure_trusted_is_pure() {
    let src = r#"
module F
foreign "C" pure fn llabs(x: Int) -> Int
val main = { llabs(1) }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module_with_options(
        &hir,
        NameVisibility::default(),
        InferOptions {
            trust_foreign_pure: true,
            recovering: false,
        },
    )
    .expect("trusted pure");
    assert!(matches!(
        typed.fun_types.get("llabs"),
        Some(Type::Fun(_, _, Effect::Pure))
    ));
}

#[test]
fn foreign_without_pure_is_io() {
    let src = r#"
module F
foreign "C" fn getenv(s: String) -> String
val main = { getenv("PATH") }
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let typed = infer_module(&hir).expect("infer");
    assert!(matches!(
        typed.fun_types.get("getenv"),
        Some(Type::Fun(_, _, Effect::Io))
    ));
}

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
            body: Expr::BuiltinCall {
                name: Builtin::ListGet,
                args: vec![Expr::Int(1, span)], // missing index
                span,
            },
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

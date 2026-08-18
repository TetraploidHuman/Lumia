use super::*;

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
fn dump_num_vec2_add_ty() {
    use crate::{display_type, infer_module};
    use lumia_hir::lower_module;
    use lumia_syntax::parse_module;
    let src = r#"
module M
type Vec2 { val x val y }
trait Num {
    val add = { self, other -> self }
}
instance Num for Vec2 {
    val add = { self, other ->
        Vec2 { x = self.x + other.x, y = self.y + other.y }
    }
}
val main = {
    val a = Vec2 { x = 1.5, y = 2.0 }
    val b = Vec2 { x = 0.5, y = 1.0 }
    val s = a + b
    s
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("hir");
    let typed = infer_module(&hir).expect("ty");
    let t = typed.fun_types.get("__Num_Vec2_add").cloned().expect("fun");
    let sch = typed.fun_schemes.get("__Num_Vec2_add").cloned();
    println!("ty = {}", display_type(&t, &[]));
    println!("ty dbg = {:?}", t);
    println!("scheme = {:?}", sch);
    if let Some(s) = &sch {
        println!(
            "needs_mono={} num_vars={:?} vars={:?}",
            s.needs_mono(),
            s.num_vars,
            s.vars
        );
        println!("scheme ty = {}", display_type(&s.ty, &s.num_vars));
    }
}

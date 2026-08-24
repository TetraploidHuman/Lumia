use super::*;

#[test]
fn pure_may_construct_io_thunk() {
    let src = r#"
module T
import lumi.io.{println}
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
import lumi.io.{println}
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
import lumi.io.{println}
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
import lumi.io.{println}
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
fn hof_picks_up_callback_io() {
    let src = r#"
module Hof
import lumi.io.{println}
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
/// `if` arms joining Pure/Io function values must lub to Io on the caller.
fn if_branches_io_vs_pure_fun_marks_caller_or_rejects() {
    let src = r#"
module Hole
import lumi.io.{println}
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
import lumi.io.{println}
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
import lumi.io.{println}
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

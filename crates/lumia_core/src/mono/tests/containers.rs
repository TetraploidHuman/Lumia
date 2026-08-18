use crate::compile_source_to_core;
use lumia_ty::Type;

#[test]
fn unwrapor_err_float_default_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Result { Ok(v) Err(e) }
val unwrapOr = { r, default ->
    r match {
        Ok(x) -> x
        Err(_) -> default
    }
}
val main = {
    println(unwrapOr(Err("e"), 1.5))
}
"#,
    )
    .expect("core");
    assert!(
        core.functions
            .iter()
            .any(|f| f.name.starts_with("unwrapOr$") && matches!(f.ret_ty, lumia_ty::Type::Float)),
        "need Float unwrapOr$ for Err+1.5, got {:?}",
        core.functions
            .iter()
            .filter(|f| f.name.contains("unwrapOr"))
            .map(|f| (&f.name, &f.ret_ty))
            .collect::<Vec<_>>()
    );
}

#[test]
fn unwrapor_none_float_default_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val unwrapOr = { opt, default ->
    opt match {
        Some(x) -> x
        None -> default
    }
}
val main = {
    println(unwrapOr(None, 1.5))
    println(unwrapOr(None, true))
}
"#,
    )
    .expect("core");
    assert!(
        core.functions
            .iter()
            .any(|f| f.name.starts_with("unwrapOr$") && matches!(f.ret_ty, lumia_ty::Type::Float)),
        "need Float unwrapOr$ for None+1.5, got {:?}",
        core.functions
            .iter()
            .filter(|f| f.name.contains("unwrapOr"))
            .map(|f| (&f.name, &f.ret_ty))
            .collect::<Vec<_>>()
    );
    assert!(
        core.functions
            .iter()
            .any(|f| f.name.starts_with("unwrapOr$") && matches!(f.ret_ty, lumia_ty::Type::Bool)),
        "need Bool unwrapOr$ for None+true"
    );
}

#[test]
fn unwrapor_some_float_fun_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Option { Some(v) None }
val unwrapOr = { opt, default ->
    opt match {
        Some(x) -> x
        None -> default
    }
}
val main = {
    val f = unwrapOr(Some({ x -> x + 1.0 }), { x -> x })
    println(f(1.5))
}
"#,
    )
    .expect("core");
    let u = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("unwrapOr$"))
        .expect("clone");
    assert!(
        matches!(&u.ret_ty, Type::Fun(ps, r, _) if ps.first().is_some_and(|p| matches!(p, Type::Float)) && matches!(r.as_ref(), Type::Float)),
        "unwrapOr$ should return Fun(Float)->Float, got {:?}",
        u.ret_ty
    );
}

#[test]
fn tuple_both_float_mono() {
    let core = crate::compile_source_to_core(
        r#"
module Main
import std.io.{println}
val both = { p, f -> (f(p.0), f(p.1)) }
val main = {
  val q = both((1.5, 2.25), { x -> x * 2.0 })
  println(q.0)
  println(q.1)
}
"#,
    )
    .expect("compile");
    let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
    eprintln!("funs={names:?}");
    for f in core
        .functions
        .iter()
        .filter(|f| f.name.starts_with("both") || f.name.contains("__lam"))
    {
        eprintln!("  {} params={:?} ret={:?}", f.name, f.param_tys, f.ret_ty);
    }
}

#[test]
fn tuple_id_float_mono() {
    let core = crate::compile_source_to_core(
        r#"
module Main
import std.io.{println}
val id = { p -> p }
val main = {
  val p = id((1.5, 2))
  println(p.0)
}
"#,
    )
    .expect("compile");
    let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
    eprintln!("funs={names:?}");
    for f in core.functions.iter().filter(|f| f.name.starts_with("id")) {
        eprintln!("  {} params={:?} ret={:?}", f.name, f.param_tys, f.ret_ty);
    }
    assert!(
        names.iter().any(|n| *n != "id" && n.starts_with("id")),
        "expected id$… mono clone for Float tuple, funs={names:?}"
    );
}

#[test]
fn unwrap_task_float_mono_clone() {
    let core = crate::compile_source_to_core(
        r#"
module Main
import std.io.{println}
val unwrapTask = { t -> t.join() }
val main = {
  scope {
    val tf = spawn { 1.5 }
    println(unwrapTask(tf))
  }
}
"#,
    )
    .expect("compile");
    let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names
            .iter()
            .any(|n| n.contains("unwrapTask") && n.contains("Task_Float")),
        "expected unwrapTask$Task_Float clone, funs={names:?}"
    );
    let clone = core
        .functions
        .iter()
        .find(|f| f.name.contains("unwrapTask") && f.name.contains("Task_Float"))
        .expect("clone");
    assert!(
        matches!(clone.ret_ty, lumia_ty::Type::Float),
        "clone ret should be Float, got {:?}",
        clone.ret_ty
    );
    assert!(
        matches!(
            clone.param_tys.first(),
            Some(lumia_ty::Type::Task(e)) if matches!(e.as_ref(), lumia_ty::Type::Float)
        ),
        "clone param0 should be Task[Float], got {:?}",
        clone.param_tys
    );
}

#[test]
fn spawn_identity_float_icall_rewritten() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val f = spawn { { x -> x } }.join()
        println(f(1.5))
    }
}
"#,
    )
    .expect("core");
    assert!(
        core.functions.iter().any(|f| f.name == "__lam_0$Float"
            || f.name.ends_with("$Float") && f.name.contains("__lam_0")),
        "expected __lam_0$Float, got {:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let mut calls = vec![];
    let mut icalls = 0u32;
    fn walk(b: &crate::ir::Block, calls: &mut Vec<String>, icalls: &mut u32) {
        for op in &b.ops {
            let v = match op {
                crate::ir::Op::Let { value, .. } => value,
                _ => continue,
            };
            match v {
                crate::ir::Value::Call { fun, .. } => calls.push(fun.name.clone()),
                crate::ir::Value::IndirectCall { .. } => *icalls += 1,
                crate::ir::Value::If {
                    then_block,
                    else_block,
                    ..
                } => {
                    walk(then_block, calls, icalls);
                    walk(else_block, calls, icalls);
                }
                _ => {}
            }
        }
    }
    walk(&main.body, &mut calls, &mut icalls);
    assert!(
        calls.iter().any(|c| c.contains("$Float")),
        "expected Call(__lam_$Float), calls={calls:?} icalls={icalls}"
    );
    assert_eq!(
        icalls, 0,
        "identity apply should be direct Call, icalls={icalls}"
    );
}

#[test]
fn spawn_option_map_unwrapor_float_key() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Option { Some(v) None }
val unwrapOr = { opt, default ->
    opt match {
        Some(x) -> x
        None -> default
    }
}
val optionMap = { opt, f ->
    opt match {
        None -> None
        Some(x) -> Some(f(x))
    }
}
val main = {
    scope {
        val o = spawn { optionMap(Some(1.5), { x -> x * 3.0 }) }.join()
        println(unwrapOr(o, 0.0))
    }
}
"#,
    )
    .expect("core");
    let u = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("unwrapOr$"))
        .unwrap_or_else(|| {
            panic!(
                "no unwrapOr$ in {:?}",
                core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
            )
        });
    assert!(
        u.name.contains("Float") && !u.name.contains("Option_Int"),
        "unwrapOr should specialize Option[Float], got {}",
        u.name
    );
}

#[test]
fn flatten_nested_option_unwrapor_int() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Option { Some(v) None }
val flatten = { o ->
    o match {
        Some(inner) -> inner
        None -> None
    }
}
val unwrapOr = { opt, default ->
    opt match {
        Some(x) -> x
        None -> default
    }
}
val main = {
    println(unwrapOr(flatten(Some(Some(3))), 0))
}
"#,
    )
    .expect("core");
    let flat = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("flatten$"))
        .expect("flatten$");
    let pmap = super::super::ret_ty::param_ty_map(flat);

    let empty_traits = Default::default();
    let index = super::super::fun_index::FunIndex::new(
        &core.functions,
        &core.sum_max_arity,
        &empty_traits,
        core.channel_elem_hint.as_ref(),
    );
    let body_ty =
        super::super::ret_ty::block_result_fixed_ty(&flat.body, &index, &empty_traits, &pmap);
    assert!(
        matches!(
            &body_ty,
            Some(lumia_ty::Type::Adt { name, params })
                if lumia_hir::is_option(name)
                    && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Int))
        ),
        "block_result_fixed_ty should be Option[Int], got {body_ty:?}"
    );
    assert!(
        matches!(
            &flat.ret_ty,
            lumia_ty::Type::Adt { name, params }
                if lumia_hir::is_option(name)
                    && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Int))
        ),
        "flatten should return Option[Int], got {:?}",
        flat.ret_ty
    );
    let u = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("unwrapOr$"))
        .expect("unwrapOr$");
    assert!(
        !u.name.contains("Option_Option"),
        "flatten should yield Option[Int]; unwrapOr got {}",
        u.name
    );
    assert!(
        u.name.contains("Option_Int"),
        "expected unwrapOr$Option_Int_*, got {}",
        u.name
    );
}

#[test]
fn nested_andthen_unwrapor_float() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Option { Some(v) None }
val unwrapOr = { opt, default ->
    opt match {
        Some(x) -> x
        None -> default
    }
}
val andThen = { o, f ->
    o match {
        None -> None
        Some(x) -> f(x)
    }
}
val main = {
    val o = andThen(Some(1.5), { x -> andThen(Some(x * 2.0), { y -> Some(y + 1.0) }) })
    println(unwrapOr(o, 0.0))
}
"#,
    )
    .expect("core");
    let u = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("unwrapOr$"))
        .expect("unwrapOr$");
    assert!(
        !u.name.contains("Option_Option"),
        "nested andThen should yield Option[Float] not Option[Option[_]], got {}",
        u.name
    );
    assert!(
        u.name.contains("Float"),
        "expected Float unwrapOr clone, got {}",
        u.name
    );
}

#[test]
fn result_andthen_then_unwrapor_float_clone() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Result { Ok(v) Err(e) }
val unwrapOr = { r, default ->
    r match {
        Ok(x) -> x
        Err(_) -> default
    }
}
val andThen = { r, f ->
    r match {
        Ok(x) -> f(x)
        Err(e) -> Err(e)
    }
}
val main = {
    val r = andThen(Ok(1.5), { x -> Ok(x * 2.0) })
    println(unwrapOr(r, 0.0))
}
"#,
    )
    .expect("core");
    let and_then = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("andThen$"))
        .expect("andThen$");
    assert!(
        matches!(
            &and_then.ret_ty,
            Type::Adt { name, params }
                if lumia_hir::is_result(name)
                    && params.first().is_some_and(|p| matches!(p, Type::Float))
                    && !params.iter().any(|p| matches!(p, Type::Var(_)))
        ),
        "andThen$ ret must be ground Result[Float,…], got {:?}",
        and_then.ret_ty
    );
    assert!(
        core.functions
            .iter()
            .any(|f| f.name.starts_with("unwrapOr$")),
        "unwrapOr$ clone required after andThen Float payload"
    );
}

#[test]
fn andthen_float_payload_ret_tys() {
    let core = compile_source_to_core(
        r#"
module M
type Option { Some(v) None }
val andThen = { o, f ->
    o match {
        None -> None
        Some(x) -> f(x)
    }
}
val times2 = { x -> Some(x * 2.0) }
val main = {
    andThen(Some(1.5), times2) match {
        Some(v) -> v
        None -> 0.0
    }
}
"#,
    )
    .expect("core");
    let times2 = core
        .functions
        .iter()
        .find(|f| f.name == "times2")
        .expect("times2");
    assert!(
        matches!(
            &times2.ret_ty,
            Type::Adt { name, params } if lumia_hir::is_option(name)
                && params.first().is_some_and(|p| matches!(p, Type::Float))
        ),
        "times2 ret should be Option[Float], got {:?}",
        times2.ret_ty
    );
    let and_then = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("andThen$"))
        .expect("andThen mono clone");
    assert!(
        matches!(
            &and_then.ret_ty,
            Type::Adt { name, params } if lumia_hir::is_option(name)
                && params.first().is_some_and(|p| matches!(p, Type::Float))
        ),
        "andThen$ clone ret should be Option[Float], got {:?}",
        and_then.ret_ty
    );
    assert!(
        matches!(
            and_then.param_tys.first(),
            Some(Type::Adt { name, params }) if lumia_hir::is_option(name)
                && params.first().is_some_and(|p| matches!(p, Type::Float))
        ),
        "andThen$ param0 should be Option[Float], got {:?}",
        and_then.param_tys
    );
}

use crate::compile_source_to_core;
use crate::ir::{Op, Value};
use lumia_ty::Type;

#[test]
fn option_id_alt_directizes_or_keeps_fun_icall() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    val o = Some({ x -> x })
    val f = o alt { x -> x }
    println(f(2.5))
}
"#,
    )
    .expect("core");
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let mut icall = 0usize;
    let mut call_float = 0usize;
    let mut alloc_fields: Vec<usize> = vec![];
    crate::for_each_block_dfs(&main.body, &mut |b| {
        for op in &b.ops {
            match op {
                Op::Let {
                    value: Value::IndirectCall { .. },
                    ..
                } => icall += 1,
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun.contains("$Float") || fun.starts_with("__lam_") => {
                    call_float += 1;
                }
                Op::Let {
                    value: Value::AllocAdt { fields, .. },
                    ..
                } => alloc_fields.push(fields.len()),
                _ => {}
            }
        }
    });
    assert!(
        call_float >= 1 || icall == 0,
        "expected Call(__lam*$Float) for Some(id) alt id; icall={icall} call_float={call_float} alloc_fields={alloc_fields:?} body={:?}",
        main.body
    );
}

#[test]
fn option_map_list_len_clone_ret_is_int_not_list() {
    // `{ xs -> xs.len() }` must keep Int ret on `$List_Int` clones; merging
    // MonoKey List over body Int made optionMap→unwrapOr retain on `3`.
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
  println(unwrapOr(optionMap(Some(listOf(1, 2, 3)), { xs -> xs.len() }), 0))
}
"#,
    )
    .expect("core");
    let lam = core
        .functions
        .iter()
        .find(|f| f.name == "__lam_0$List_Int")
        .expect("__lam_0$List_Int");
    assert!(
        matches!(lam.ret_ty, Type::Int),
        "ListLen clone ret must be Int, got {:?}",
        lam.ret_ty
    );
    let om = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("optionMap$"))
        .expect("optionMap$");
    assert!(
        matches!(
            &om.ret_ty,
            Type::Adt { name, params }
                if lumia_hir::is_option(name) && matches!(params.first(), Some(Type::Int))
        ),
        "optionMap ret must be Option[Int], got {:?}",
        om.ret_ty
    );
    let u = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("unwrapOr$"))
        .expect("unwrapOr$");
    assert!(
        u.name.contains("Option_Int") && !u.name.contains("List"),
        "unwrapOr should be Option[Int], got {}",
        u.name
    );
}

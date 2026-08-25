//! Core IR — ANF / SSA-ish form used by optimization and codegen.

mod ir;
mod lambda_lift;
mod lower;
mod mono;
mod pipeline;
mod value_ty;
mod visit;

pub use ir::{
    format_module, max_local_in_block, max_local_in_fun, rewrite_block_locals, AdtRepr, Block,
    CoreFun, CoreModule, ListRepr, Local, MapRepr, MemoTf, Op, SetRepr, Value,
};
pub use lower::{lower_hir, lower_hir_with_schemes};
pub use pipeline::{
    compile_file_to_core, compile_source_to_core, compile_source_to_core_with_options,
    compile_source_to_core_with_parallel, FrontendOptions,
};
pub use value_ty::{
    infer_value_ty, infer_value_ty_ctx, list_par_map_elem_ty, value_alloc_may_heap,
    CodegenTypeTables, HeapPolicy, InferValueCtx,
};
pub use visit::{
    block_calls, block_has_io, collect_uses_in_value, count_ops, for_each_block_dfs,
    for_each_local, for_each_local_mut, for_each_nested_block, for_each_nested_block_mut,
    for_each_op_value_mut, has_assign_or_name, has_early_return, map_value_locals,
    max_local_in_value, rewrite_value_locals,
};

#[cfg(test)]
mod tests {
    use super::*;
    use lumi_hir::lower_module;
    use lumi_syntax::parse_module;
    use lumi_ty::{infer_module, Type};

    #[test]
    fn nested_identity_lambda_ret_ty_is_heap() {
        let core = compile_source_to_core(
            r#"
module M
val main = {
    val id = { s -> s }
    id("hi")
}
"#,
        )
        .expect("core");
        let lam = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_"))
            .unwrap_or_else(|| {
                panic!(
                    "expected lifted lambda, funs={:?}",
                    core.functions
                        .iter()
                        .map(|f| (&f.name, &f.ret_ty))
                        .collect::<Vec<_>>()
                )
            });
        assert!(
            !matches!(
                lam.ret_ty,
                Type::Int | Type::Bool | Type::Float | Type::Unit
            ),
            "nested identity lambda must not claim scalar ret_ty (got {:?})",
            lam.ret_ty
        );
    }

    #[test]
    fn isok_ret_ty_is_bool() {
        let core = compile_source_to_core(
            r#"
module M
val println(x) = { __println(x) }
val isOk(r) = {
    r match {
        Ok(_) -> true
        Err(_) -> false
    }
}
val main = { println(isOk(Ok(1))) }
"#,
        )
        .expect("core");
        let is_ok = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("isOk"))
            .expect("isOk");
        assert_eq!(is_ok.ret_ty, Type::Bool, "isOk ret_ty={:?}", is_ok.ret_ty);
        let is_ok_mono = core
            .functions
            .iter()
            .find(|f| f.name == "isOk$Result_Int")
            .expect("isOk mono");
        assert_eq!(
            is_ok_mono.ret_ty,
            Type::Bool,
            "isOk mono ret_ty={:?}",
            is_ok_mono.ret_ty
        );
        let println_names: Vec<_> = core
            .functions
            .iter()
            .filter(|f| f.name.contains("println"))
            .map(|f| f.name.clone())
            .collect();
        assert!(
            println_names.iter().any(|n| n.contains("$Bool")),
            "println clones: {println_names:?}"
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert!(
            calls.iter().any(|c| c.contains("$Bool")),
            "main println calls: {calls:?}"
        );
    }

    #[test]
    fn wide_product_float_field_println() {
        let core = compile_source_to_core(
            r#"
module M
val println(x) = { __println(x) }
type Wide {
    val i0
    val i1
    val i2
    val f
}
val mk(x) = { Wide { i0 = 0, i1 = 0, i2 = 0, f = x } }
val main = {
    println(mk(1.25).f)
}
"#,
        )
        .expect("core");
        let mk = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("mk$"))
            .expect("mk mono");
        assert!(
            matches!(
                &mk.ret_ty,
                Type::Adt { params, .. }
                    if params.get(3).is_some_and(|p| matches!(p, Type::Float))
            ),
            "mk$Float ret should have Float at field 3, got {:?}",
            mk.ret_ty
        );
        let println_clones: Vec<_> = core
            .functions
            .iter()
            .filter(|f| f.name.contains("println"))
            .map(|f| f.name.clone())
            .collect();
        assert!(
            println_clones.iter().any(|n| n.contains("$Float")),
            "expected println$Float clone, funs={println_clones:?}"
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun.starts_with("println") => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert!(
            calls.iter().any(|c| c.contains("$Float")),
            "println calls={calls:?}"
        );
    }

    #[test]
    fn headsum_float_ret_specializes_println() {
        let core = compile_source_to_core(
            r#"
module M
val println(x) = { __println(x) }
val zeros(n) = {
    var xs = listOf()
    var i = 0
    for i < n {
        xs = xs.append(0.0)
        i = i + 1
    }
    xs
}
val headSum(xs, n) = {
    var s = 0.0
    var i = 0
    for i < n {
        s = s + xs.get(i)
        i = i + 1
    }
    s
}
val main = {
    var xs = zeros(2)
    xs = xs.set(0, 0.668)
    xs = xs.set(1, 0.460)
    println(headSum(xs, 2))
}
"#,
        )
        .expect("core");
        let hs = core
            .functions
            .iter()
            .find(|f| f.name == "headSum")
            .expect("headSum");
        assert!(
            matches!(hs.ret_ty, Type::Float),
            "headSum.ret_ty={:?}",
            hs.ret_ty
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun.starts_with("println") => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert!(
            calls.iter().any(|c| c.contains("$Float")),
            "println calls={calls:?}"
        );
    }

    #[test]
    fn println_mono_after_hof_apply_float_exact_example() {
        // Mirrors examples/hof_float_apply.lm (local println stand-in for lumi.io).
        let core = compile_source_to_core(
            r#"
module HofFloatApply
val println(x) = { __println(x) }
val dbl(x) = x + x
val apply(f, x) = f(x)
val main = {
    println(dbl(1.5))
    println(apply(dbl, 1.5))
    println(apply(dbl, 2.0))
}
"#,
        )
        .expect("core");
        let dbl = core.functions.iter().find(|f| f.name == "dbl").expect("dbl");
        assert!(dbl.scheme_poly, "dbl should be scheme_poly");
        let apply_clone = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("apply$"))
            .expect("apply mono");
        assert!(
            matches!(apply_clone.ret_ty, Type::Float),
            "apply ret={:?} name={}",
            apply_clone.ret_ty,
            apply_clone.name
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let println_calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } if fun.starts_with("println") => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            println_calls.iter().filter(|c| c.contains("$Float")).count(),
            3,
            "all three println sites should be $Float, got {println_calls:?}"
        );
    }

    #[test]
    fn println_mono_after_hof_apply_float() {
        let core = compile_source_to_core(
            r#"
module M
val println(x) = { __println(x) }
val dbl(x) = x + x
val apply(f, x) = f(x)
val main = {
    println(apply(dbl, 1.5))
}
"#,
        )
        .expect("core");
        let apply_clone = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("apply$"))
            .expect("apply mono");
        assert!(
            matches!(apply_clone.ret_ty, Type::Float),
            "apply clone ret={:?}",
            apply_clone.ret_ty
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert!(
            calls.iter().any(|c| c.contains("println$Float")),
            "expected println$Float after apply, calls={calls:?}; funs={:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn println_mono_clones_for_float_hof() {
        let core = compile_source_to_core(
            r#"
module M
val println(x) = { __println(x) }
val main = {
    val dbl = { x -> x + x }
    println(dbl(1))
    println(dbl(1.5))
}
"#,
        )
        .expect("core");
        let names: Vec<_> = core
            .functions
            .iter()
            .filter(|f| f.name.contains("println"))
            .map(|f| f.name.clone())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("$Float")),
            "expected println$Float, got {names:?}"
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert!(
            calls.iter().any(|c| c.contains("$Float")),
            "main println calls: {calls:?}"
        );
    }

    #[test]
    fn println_mono_clones_for_unit_and_bool() {
        let core = compile_source_to_core(
            r#"
module M
val println(x) = { __println(x) }
val side(x) = { println(x) }
val main = {
    println(side(1))
    println(true)
}
"#,
        )
        .expect("core");
        let names: Vec<_> = core
            .functions
            .iter()
            .filter(|f| f.name.contains("println"))
            .map(|f| f.name.clone())
            .collect();
        assert!(
            names.iter().any(|n| n.contains("$Bool")),
            "expected println$Bool mono clone, got {names:?}"
        );
        assert!(
            names.iter().any(|n| n.contains("$Unit")),
            "expected println$Unit mono clone, got {names:?}"
        );
        let main = core.functions.iter().find(|f| f.is_main).expect("main");
        let calls: Vec<_> = main
            .body
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Let {
                    value: Value::Call { fun, .. },
                    ..
                } => Some(fun.clone()),
                _ => None,
            })
            .collect();
        assert!(
            calls.iter().any(|c| c.contains("$Bool")),
            "main should call println$Bool, calls={calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.contains("$Unit")),
            "main should call println$Unit, calls={calls:?}"
        );
    }

    #[test]
    fn scheme_poly_top_level_dbl() {
        let src = r#"
module M
val dbl = { x -> x + x }
val main = {
    dbl(1)
    dbl(1.5)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("hir");
        let typed = infer_module(&hir).expect("ty");
        assert!(
            typed
                .fun_schemes
                .get("dbl")
                .map(|s| s.needs_mono())
                .unwrap_or(false),
            "dbl should have a polymorphic scheme: {:?}",
            typed.fun_schemes.get("dbl")
        );
        let core = compile_source_to_core(src).expect("core");
        assert!(
            core.functions
                .iter()
                .any(|f| f.name.contains("dbl") && f.name.contains('$')),
            "expected mono clone of dbl, funs={:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn monomorphic_fun_not_cloned() {
        let src = r#"
module M
val flag(b) = if b { true } else { false }
val main = {
    flag(true)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("hir");
        let typed = infer_module(&hir).expect("ty");
        assert!(
            !typed
                .fun_schemes
                .get("flag")
                .map(|s| s.needs_mono())
                .unwrap_or(true),
            "flag(Bool) should be mono scheme: {:?}",
            typed.fun_schemes.get("flag")
        );
        let core = compile_source_to_core(src).expect("core");
        assert!(
            core.functions
                .iter()
                .any(|f| f.name == "flag" && !f.scheme_poly),
            "flag should not be scheme_poly: {:?}",
            core.functions
                .iter()
                .map(|f| (&f.name, f.scheme_poly))
                .collect::<Vec<_>>()
        );
        assert!(
            !core.functions.iter().any(|f| f.name.contains('$')),
            "monomorphic flag should not clone: {:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lifted_io_lambda_marks_effect() {
        let core = compile_source_to_core(
            r#"
module M
val println(x) = { __println(x) }
val log = { x -> println(x) }
val main = {
    val f = { -> log(1) }
    f()
}
"#,
        )
        .expect("core");
        let lam = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_"))
            .expect("lifted lambda");
        assert!(
            lam.effect.has_io(),
            "__lam that calls IO must be effect.io; got {:?}",
            lam.effect
        );
    }
}

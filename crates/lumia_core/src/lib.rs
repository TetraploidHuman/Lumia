//! Core IR — 树形 ANF / 伪 SSA，供优化与 codegen 使用（非真 CFG）。

mod ir;
mod lambda_lift;
mod lower;
mod mono;
mod ops;
mod pipeline;
mod value_ty;
mod visit;

pub use ir::{
    format_module, max_local_in_block, max_local_in_fun, rewrite_block_locals, AdtRepr, Block,
    CoreFun, CoreModule, ForeignAbi, FunKind, ListRepr, Local, MapRepr, MemoTf, Op, SetRepr,
    Value,
};
pub use ops::{CoreBinOp, CoreUnOp};
/// Mid-end may share ABI contract constants with rt/codegen via `lumia_abi`.
pub use lumia_abi::SMALL_CONTAINER_MAX;
pub use lower::lower_hir_with_schemes;
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
    use lumia_hir::lower_module;
    use lumia_syntax::parse_module;
    use lumia_ty::{infer_module, Type};

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
            .find(|f| f.is_lifted_lambda())
            .unwrap_or_else(|| {
                panic!(
                    "expected lifted lambda, funs={:?}",
                    core.functions
                        .iter()
                        .map(|f| (&f.name, f.kind, &f.ret_ty))
                        .collect::<Vec<_>>()
                )
            });
        assert!(
            lam.name.starts_with("__lam_"),
            "historical name prefix still used for lifted lambdas"
        );
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
import std.io.{println}
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
            .find(|f| f.is_lifted_lambda())
            .expect("lifted lambda");
        assert!(
            lam.effect.has_io(),
            "__lam that calls IO must be effect.io; got {:?}",
            lam.effect
        );
    }
}

//! Core IR — 树形 ANF / 伪 SSA，供优化与 codegen 使用（非真 CFG）。
#![allow(clippy::too_many_arguments)] // HOF / ABI walkers
#![allow(clippy::type_complexity)] // InferValueCtx callback types
#![allow(clippy::collapsible_match)] // nested Op/Value arms

mod ir;
mod lambda_lift;
mod lower;
mod module_tables;
mod mono;
mod ops;
mod pipeline;
mod sr_pattern;
mod value_ty;
mod visit;

pub use ir::{
    format_module, AdtRepr, Block, CoreFun, CoreModule, ForeignAbi, FunKind, ListRepr, Local,
    MapRepr, MemoTf, Op, SetRepr, Value,
};
pub use module_tables::{core_fun_is_param0_identity, ModuleTables};
pub use ops::{CoreBinOp, CoreUnOp};
/// Mid-end may share ABI contract constants with rt/codegen via `lumia_abi`.
pub use lumia_abi::SMALL_CONTAINER_MAX;
pub use lower::lower_hir_with_schemes;
pub use pipeline::{
    compile_file_to_core, compile_source_to_core, compile_source_to_core_with_options,
    compile_source_to_core_with_parallel, FrontendOptions,
};
pub use value_ty::{
    infer_value_ty, infer_value_ty_ctx, join_abi_tys, list_par_map_elem_ty, prefer_concrete_heap_ty,
    type_may_heap, value_alloc_may_heap, CodegenTypeTables, HeapPolicy, InferValueCtx, JoinAbiKind,
};
pub use visit::{
    block_calls, block_has_io, collect_alloc_closure_caps, collect_alloc_closure_env_funs,
    collect_assigns, collect_assigned_names, collect_call_names_in, collect_closure_cap_funrefs,
    collect_defined_locals, collect_float_cap_indices, collect_loops, collect_slot_names,
    collect_ssa_live_refs, collect_uses_in_value, count_ops, first_direct_loop, for_each_block_dfs,
    for_each_let_value, for_each_let_value_ctrl, for_each_local, for_each_local_mut,
    for_each_nested_block, for_each_nested_block_mut, for_each_op_value_mut, has_assign_or_name,
    has_early_return, local_let_matches, map_value_locals, max_local_in_block, max_local_in_fun,
    max_local_in_value, rewrite_block_locals, rewrite_value_locals,
};
pub use sr_pattern::{
    add_name_other, body_assigns_const, body_assigns_name_div_const,
    body_assigns_name_mul_const_plus_const, body_assigns_rem, body_assigns_unit_inc,
    body_assigns_zero_or_false, collect_leaf_defs, const_of, has_float_approx,
    has_float_binop_with_const, header_ge_const, header_gt_const, header_gt_eq, header_le_const,
    header_lt_bound, header_lt_const, header_name_sq_cmp, header_name_sq_le_name,
    is_add_name_plus_any, is_add_name_plus_name, is_affine_ik1, is_affine_kj1, is_list_get,
    is_list_set, is_name_add_const, is_name_mul_const, is_name_mul_const_plus_const,
    is_name_mul_name, is_name_ne_zero, is_name_div_const, is_name_rem_eq_const,
    is_nontrivial_add_or_sub, is_nontrivial_arith, is_rem, is_small_factor_mul_nonneg, is_unit_inc,
    is_unit_inc_value, is_unit_step, local_is_zero_or_false, name_ne_zero, name_of,
    acc_add_rem_const_mod, rem_const_mod, rem_eq_zero_names, rem_eq_zero_operands, same_local,
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
        // Dual-track lock-in: lower/mono uses `$Float` (or similar), not opt `$c_`.
        assert!(
            core.functions.iter().any(|f| {
                f.mono_of.as_deref() == Some("dbl") && !f.name.contains("$c_")
            }),
            "type mono clones must set mono_of and avoid `$c_` (opt SpecializeConst), funs={:?}",
            core.functions
                .iter()
                .map(|f| (&f.name, &f.mono_of))
                .collect::<Vec<_>>()
        );
        assert!(
            !core.functions.iter().any(|f| f.name.contains("$c_")),
            "lower must not invent SpecializeConst `$c_` names"
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

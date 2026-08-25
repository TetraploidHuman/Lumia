//! Core IR — ANF / SSA-ish form used by optimization and codegen.

mod ir;
mod lambda_lift;
mod lower;
mod mono;
mod pipeline;
mod dense_f64_match;
mod sr_pattern;
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
pub use dense_f64_match::{
    body_has_gemv_inner, for_each_def_and_let, for_each_let, fun_has_add_shape, fun_has_addmm_shape,
    fun_has_axpy_shape, fun_has_clamp_shape, fun_has_copy_shape, fun_has_fill_shape,
    fun_has_gemv_t_shape, fun_has_mul_shape, fun_has_scale_shape, fun_has_sub_shape, is_list_get,
    is_list_set, is_nontrivial_add_or_sub, is_nontrivial_arith, is_unit_inc_value, list_arg_is,
    match_add_fun, match_addmm_fun, match_axpy_fun, match_clamp_fun, match_copy_fun, match_fill_fun,
    match_gemv_fun, match_gemv_t_fun, match_mul_fun, match_scale_fun, match_sub_fun, mentions_local,
    DenseAddmm, DenseAxpy, DenseBin3, DenseClamp, DenseCopy, DenseFill, DenseGemv, DenseScale,
};
pub use sr_pattern::{
    first_assign_from_local, first_loop, header_lt_bound, is_unit_inc, name_of, same_local,
};
pub use value_ty::{
    infer_value_ty, infer_value_ty_ctx, list_par_map_elem_ty, value_alloc_may_heap,
    CodegenTypeTables, HeapPolicy, InferValueCtx,
};
pub use visit::{
    block_calls, block_has_io, collect_leaf_defs, collect_loop_triples, collect_uses_in_value,
    count_ops, for_each_block_dfs, for_each_local, for_each_local_mut, for_each_nested_block,
    for_each_nested_block_mut, for_each_op_value_mut, has_assign_or_name, has_early_return,
    map_value_locals, max_local_in_value, rewrite_value_locals,
};

#[cfg(test)]
mod integration_tests;

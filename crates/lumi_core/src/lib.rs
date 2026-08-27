//! Core IR — ANF / SSA-ish form used by optimization and codegen.

mod dense_f64_match;
mod ir;
mod lambda_lift;
mod lower;
mod mono;
mod pipeline;
mod sr_pattern;
mod type_join;
mod value_ty;
mod visit;

pub use dense_f64_match::{
    dense_f64_rt_symbol, for_each_def_and_let, is_list_get, is_list_set, is_nontrivial_add_or_sub,
    list_arg_is, match_add_fun, match_addmm_fun, match_axpy_fun, match_clamp_fun, match_copy_fun,
    match_dense_f64_fun, match_fill_fun, match_gemv_fun, match_gemv_t_fun, match_l2_norm_fun,
    match_l2_normalize_fun, match_mean_fun, match_mul_fun, match_scale_fun, match_softmax_fun,
    match_std_fun, match_sub_fun, match_sum_sq_fun, match_zeros_fun, mentions_local, DenseAddmm,
    DenseAxpy, DenseBin3, DenseClamp, DenseCopy, DenseF64Match, DenseFill, DenseGemv, DenseScale,
};
pub use ir::{
    format_module, max_local_in_block, max_local_in_fun, rewrite_block_locals, AdtRepr, Block,
    CoreFun, CoreModule, ListRepr, Local, MapRepr, MemoTf, Op, SetRepr, Value,
};
pub use lower::{lower_hir, lower_hir_with_schemes};
pub use pipeline::{
    compile_file_to_core, compile_source_to_core, compile_source_to_core_with_options,
    compile_source_to_core_with_parallel, FrontendOptions,
};
pub use sr_pattern::{
    acc_add_const_inc, acc_add_has_name, body_assigns_const, body_iv_unit_inc, const_int,
    first_assign_from_local, first_loop, header_dd_le_n, header_gt1_iv, header_le_const,
    header_lt_bound, header_lt_const, is_add_name_plus_name, is_affine_row_col_plus1,
    is_name_mul_const, is_unit_inc, match_nested_loop, name_of, same_local, split_acc_add,
    HeaderConstFn, NestedLoop,
};
pub use value_ty::{
    infer_value_ty, infer_value_ty_ctx, list_par_map_elem_ty, value_alloc_may_heap,
    CodegenTypeTables, HeapPolicy, InferValueCtx,
};
pub use visit::{
    block_calls, block_has_io, collect_leaf_defs, collect_loop_triples, collect_uses_in_value,
    count_ops, for_each_block_dfs, for_each_let, for_each_local, for_each_local_mut,
    for_each_nested_block, for_each_nested_block_mut, for_each_op_value_mut, has_assign_or_name,
    has_early_return, map_value_locals, max_local_in_value, rewrite_value_locals,
};

#[cfg(test)]
mod integration_tests;

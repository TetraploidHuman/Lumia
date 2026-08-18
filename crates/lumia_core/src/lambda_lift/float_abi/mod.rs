//! Float ABI inference for lifted lambdas.

mod helpers;
mod local_heap;
mod mark_float;

pub use crate::value_ty::prefer_concrete_heap_ty;

pub(super) use helpers::{
    block_result_callee_ty, block_result_fun_ty, block_result_icall_cap_ty,
    block_result_icall_cap_ty_by_index, block_result_known_hof_ty, HofSets,
};
#[cfg(test)]
pub(super) use helpers::{is_apply_hof, is_compose_hof, is_id_hof};
pub(crate) use local_heap::{block_result_heap_ty, block_result_heap_ty_caps};
pub(crate) use mark_float::collect_fun_cap_tys;
pub(super) use mark_float::{
    block_result_channel_recv_ty, block_result_channel_ty, block_result_is_bool,
    block_result_is_float, block_result_is_float_seeded, block_result_is_unit,
    compute_float_locals_in_block, local_channel_recv_elem_ty, params_used_as_float_seeded,
    params_used_as_float_with_caps_seeded, value_is_float_producing,
};

#[cfg(test)]
#[path = "../float_abi_tests.rs"]
mod tests;

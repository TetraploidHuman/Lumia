//! Lumi runtime: pluggable GC ABI + first MmBackend (generational mark-sweep).
//!
//! C ABI contract used by codegen:
//! - `lumi_alloc(nbytes, type_id) -> *mut u8`
//! - `lumi_root_push(*mut *mut u8)` / `lumi_root_pop()`
//! - `lumi_write_barrier(obj, field_index, new_ptr)` — remembered-set (old→young)
//!   plus Dijkstra shade while an incremental full mark is in flight
//! - `lumi_gc_collect()` — full-heap collection (drains concurrent mark)
//!
//! Env: `LUMI_GC_INCREMENTAL=0|false|off|stw` forces classic STW full collect
//! (default: incremental concurrent full mark).
//! - `lumi_println_int(i64)` / `lumi_println_str(*const u8, len)`
//!
//! C ABI entry points take raw pointers by design; they are not Rust `unsafe fn`
//! because the LLVM-emitted caller already treats the boundary as unchecked.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod adt_show;
mod affine2;
mod arc_free;
mod cn_kernels;
mod collatz;
mod common;
#[cfg(feature = "opt-dense-f64")]
mod dense_f64;
mod dict;
mod dispatch;
mod efe;
mod ensure;
mod eq;
mod f64_simd;
mod float_kernels;
mod gc;
mod hash_ord;
mod list;
mod map_set;
mod mm;
#[cfg(feature = "opt-memo")]
mod memo;
mod number_theory;
mod primes;
mod show;
mod string_io;

pub use adt_show::lumi_adt_register_show;
pub use common::{
    tid_base, MarkSweep, MmBackend, ObjectHeader, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
    TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA,
    TYPE_LIST_SLICE, TYPE_MAP, TYPE_MAP_ASSOC, TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V,
    TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64, TYPE_MAP_F64V, TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC,
    TYPE_SET_F64, TYPE_STRING,
};

pub use gc::{
    lumi_alloc, lumi_gc_collect, lumi_gc_full_count, lumi_gc_minor_count, lumi_gc_print_stats,
    lumi_root_pop, lumi_root_push, lumi_write_barrier,
};
pub use mm::{lumi_mm_mode, lumi_set_mm_mode, MmMode};

pub use affine2::lumi_affine2_rem_sum;
pub use cn_kernels::{
    lumi_cn_argmax, lumi_cn_axpy_clamp, lumi_cn_backproj_clamp, lumi_cn_cluster_rates,
    lumi_cn_hebbian, lumi_cn_learn_generative, lumi_cn_nucleus_step, lumi_cn_project_clamp,
    lumi_cn_update_state,
};
pub use collatz::{lumi_collatz_strided, lumi_collatz_total};
#[cfg(feature = "opt-dense-f64")]
pub use dense_f64::{
    lumi_f64_add, lumi_f64_addmm, lumi_f64_atan2, lumi_f64_axpy, lumi_f64_checksum, lumi_f64_clamp,
    lumi_f64_copy, lumi_f64_cos, lumi_f64_exp, lumi_f64_fill, lumi_f64_gemv, lumi_f64_gemv_t,
    lumi_f64_hypot, lumi_f64_l2_norm, lumi_f64_l2_normalize, lumi_f64_mean, lumi_f64_mul,
    lumi_f64_scale, lumi_f64_sin, lumi_f64_softmax, lumi_f64_sqrt, lumi_f64_std, lumi_f64_sub,
    lumi_f64_sum_sq, lumi_list_f64_zeros,
};
pub use dict::{
    lumi_dict_lookup, lumi_dict_register, lumi_dict_show, TRAIT_EQ, TRAIT_HASH, TRAIT_NUM,
    TRAIT_ORD, TRAIT_SHOW,
};
pub use dispatch::*;
pub use efe::{
    lumi_efe_action_scores, lumi_efe_apply_embodied_reflexes, lumi_efe_embodied_action_scores,
};
pub use eq::*;
pub use float_kernels::lumi_mandelbrot_checksum;
pub use hash_ord::*;
pub use list::*;
pub use map_set::*;
#[cfg(feature = "opt-memo")]
pub use memo::*;
pub use number_theory::{
    lumi_affine1_rem_sum, lumi_divisor_sum, lumi_gcd_sum, lumi_matmul_affine_checksum,
    lumi_product_rem_sum,
};
pub use primes::lumi_count_primes;
pub use show::*;
pub use string_io::*;

/// Push a Lumi frame name (nul-terminated) for trap backtraces.
#[no_mangle]
pub extern "C" fn lumi_frame_push(name: *const u8) {
    common::frame_push(name);
}

/// Pop the top Lumi frame (pair with [`lumi_frame_push`]).
#[no_mangle]
pub extern "C" fn lumi_frame_pop() {
    common::frame_pop();
}

#[cfg(test)]
mod crate_tests;

//! Lumia runtime: pluggable GC ABI + first MmBackend (generational mark-sweep).
//!
//! C ABI contract used by codegen:
//! - `lumia_alloc(nbytes, type_id) -> *mut u8`
//! - `lumia_root_push(*mut *mut u8)` / `lumia_root_pop()`
//! - `lumia_write_barrier(obj, field_index, new_ptr)` — records old→young edges
//!   in the remembered set for minor GC (no-op when `obj` is young / `new` is not)
//! - `lumia_gc_collect()` — full-heap collection
//! - `lumia_println_int(i64)` / `lumia_println_str(*const u8, len)`
//!
//! C ABI entry points take raw pointers by design; they are not Rust `unsafe fn`
//! because the LLVM-emitted caller already treats the boundary as unchecked.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod adt_show;
mod affine2;
mod collatz;
mod common;
mod dict;
mod dispatch;
mod ensure;
mod eq;
mod cn_kernels;
mod dense_f64;
mod efe;
mod float_kernels;
mod gc;
mod hash_ord;
mod list;
mod map_set;
mod memo;
mod number_theory;
mod primes;
mod show;
mod string_io;

pub use adt_show::lumia_adt_register_show;
pub use common::{
    tid_base, MarkSweep, MmBackend, ObjectHeader, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
    TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA,
    TYPE_MAP, TYPE_MAP_ASSOC, TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64,
    TYPE_MAP_F64, TYPE_MAP_F64V, TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC, TYPE_SET_F64,
    TYPE_STRING,
};

pub use gc::{lumia_alloc, lumia_gc_collect, lumia_root_pop, lumia_root_push, lumia_write_barrier};

pub use affine2::lumia_affine2_rem_sum;
pub use collatz::{lumia_collatz_strided, lumia_collatz_total};
pub use dict::{
    lumia_dict_lookup, lumia_dict_register, lumia_dict_show, TRAIT_EQ, TRAIT_HASH, TRAIT_NUM,
    TRAIT_ORD, TRAIT_SHOW,
};
pub use dispatch::*;
pub use eq::*;
pub use cn_kernels::{lumia_cn_hebbian, lumia_cn_nucleus_step};
pub use dense_f64::{
    lumia_f64_add, lumia_f64_addmm, lumia_f64_axpy, lumia_f64_checksum, lumia_f64_clamp,
    lumia_f64_copy, lumia_f64_fill, lumia_f64_gemv, lumia_f64_gemv_t, lumia_f64_l2_norm,
    lumia_f64_l2_normalize, lumia_f64_mean, lumia_f64_mul, lumia_f64_scale, lumia_f64_softmax,
    lumia_f64_std, lumia_f64_sub, lumia_f64_sum_sq, lumia_list_f64_zeros,
};
pub use efe::lumia_efe_action_scores;
pub use float_kernels::lumia_mandelbrot_checksum;
pub use hash_ord::*;
pub use list::*;
pub use map_set::*;
pub use memo::*;
pub use number_theory::{
    lumia_affine1_rem_sum, lumia_divisor_sum, lumia_gcd_sum, lumia_matmul_affine_checksum,
    lumia_product_rem_sum,
};
pub use primes::lumia_count_primes;
pub use show::*;
pub use string_io::*;

/// Push a Lumia frame name (nul-terminated) for trap backtraces.
#[no_mangle]
pub extern "C" fn lumia_frame_push(name: *const u8) {
    common::frame_push(name);
}

/// Pop the top Lumia frame (pair with [`lumia_frame_push`]).
#[no_mangle]
pub extern "C" fn lumia_frame_pop() {
    common::frame_pop();
}

#[cfg(test)]
mod crate_tests;

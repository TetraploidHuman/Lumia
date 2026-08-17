//! Lumia runtime: GC, Task/Channel scheduler, containers, memo, and domain kernels.
//!
//! C ABI contract used by codegen:
//! - `lumia_alloc(nbytes, type_id) -> *mut u8`
//! - `lumia_root_push(*mut *mut u8)` / `lumia_root_pop()`
//! - `lumia_write_barrier(obj, field_index, new_ptr)` — remembered-set (old→young)
//!   plus Dijkstra shade while an incremental full mark is in flight
//! - `lumia_gc_collect()` — full-heap collection (drains concurrent mark)
//!
//! Env: `LUMIA_GC_INCREMENTAL=0|false|off|stw|no` forces classic STW full collect;
//! `1|true|on|yes|incremental` forces incremental concurrent full mark.
//! Unrecognized values warn and keep the heap default (incremental on).
//! - `lumia_println_int(i64)` / `lumia_println_str(*const u8, len)`
//!
//! # Global init / Ordering
//!
//! Lazy singletons and atomic probe contracts are catalogued in [`globals`].
//! Prefer `OnceLock` for new process globals; document any new Ordering there.
//!
//! # Lock order
//!
//! Cross-subsystem mutex nesting (**never invert**). Ranked coarse → fine:
//!
//! 1. **heap** (`with_heap`) — process `Heap` / GC metadata
//! 2. **sched** (`with_sched`) — Task / Channel / fiber tables
//! 3. **per-mutator roots** (TLS `ROOTS` Mutex) — shadow-stack slots
//! 4. **per-mutator memo** (TLS `MEMO_TF` / `MEMO_IDX` Mutex) — memo tables
//!
//! Process-global helpers that nest under heap when GC registers/walks:
//!
//! - **mutator registry** / **memo registry** — take only while holding heap
//!   (registration and GC root enumeration: **heap → registry → per-mutator**)
//! - **channel hot path** — `with_channel_gc`: when `full_marking_fast`,
//!   **heap → sched**; otherwise sched alone (shade deferred to root remark)
//! - **memo store shade** — release memo Mutex **before** any heap shade
//!   (never **memo → heap**)
//!
//! Independent (must not nest with heap/sched, or only after releasing them):
//!
//! - **`DICTS`** / **`ADT_SHOW`** — process `Mutex`; do not hold while taking
//!   heap or sched
//!
//! Call sites that touch heap + sched must take **heap → sched** (see `gc.rs`
//! shade of sched roots; `scheduler.rs` publish under heap).
//!
//! C ABI entry points take raw pointers by design; they are not Rust `unsafe fn`
//! because the LLVM-emitted caller already treats the boundary as unchecked.
//!
//! # Tests
//!
//! Lib tests share one process `Heap` and one `SchedCore`. Parallel harness
//! threads UAF / abort (GC frees another case's objects). Always run with
//! `RUST_TEST_THREADS=1` (see `scripts/check.sh` and CI). Stress/fiber cases
//! also take an internal `sched_test_guard` for TLS scrubbing.
//! Pointer-taking C ABI is marked `unsafe extern` per subsystem (`deny` on those
//! modules); the crate-level `not_unsafe_ptr_arg_deref` allow was removed.

mod adt_show;
mod affine2;
mod cn_kernels;
mod collatz;
mod common;
mod concurrency_policy;
mod dense_f64;
mod globals;
mod heap;
mod mutator;
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
mod memo;
mod number_theory;
mod primes;
mod reentrant;
mod show;
mod string_io;
mod task;

pub use adt_show::lumia_adt_register_show;
pub use common::{
    tid_base, MarkSweep, ObjectHeader, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS,
    MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS,
    TYPE_ADT, TYPE_BYTES, TYPE_CHAR, TYPE_CHANNEL, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64,
    TYPE_LIST_IOTA, TYPE_MAP, TYPE_MAP_ASSOC, TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V,
    TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64, TYPE_MAP_F64V, TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC,
    TYPE_SET_F64, TYPE_STRING, TYPE_TASK,
};

pub use gc::{lumia_alloc, lumia_gc_collect, lumia_root_pop, lumia_root_push, lumia_write_barrier};

pub use affine2::lumia_affine2_rem_sum;
pub use cn_kernels::{
    lumia_cn_argmax, lumia_cn_axpy_clamp, lumia_cn_backproj_clamp, lumia_cn_cluster_rates,
    lumia_cn_hebbian, lumia_cn_learn_generative, lumia_cn_nucleus_step, lumia_cn_project_clamp,
    lumia_cn_update_state,
};
pub use collatz::{lumia_collatz_strided, lumia_collatz_total};
pub use dense_f64::{
    lumia_f64_add, lumia_f64_addmm, lumia_f64_atan2, lumia_f64_axpy, lumia_f64_checksum,
    lumia_f64_clamp, lumia_f64_copy, lumia_f64_cos, lumia_f64_exp, lumia_f64_fill, lumia_f64_gemv,
    lumia_f64_gemv_t, lumia_f64_hypot, lumia_f64_l2_norm, lumia_f64_l2_normalize, lumia_f64_mean,
    lumia_f64_mul, lumia_f64_scale, lumia_f64_sin, lumia_f64_softmax, lumia_f64_sqrt,
    lumia_f64_std, lumia_f64_sub, lumia_f64_sum_sq, lumia_list_f64_zeros,
};
pub use dict::{
    lumia_dict_lookup, lumia_dict_register, lumia_dict_show, TRAIT_EQ, TRAIT_HASH, TRAIT_NUM,
    TRAIT_ORD, TRAIT_SHOW,
};
pub use dispatch::{
    lumia_concat, lumia_contains, lumia_elems, lumia_get, lumia_len, lumia_remove, lumia_set,
};
pub use efe::{
    lumia_efe_action_scores, lumia_efe_apply_embodied_reflexes, lumia_efe_embodied_action_scores,
};
pub use eq::{lumia_adt_eq, lumia_eq};
pub use float_kernels::lumia_mandelbrot_checksum;
pub use hash_ord::{
    lumia_adt_ensure_unique, lumia_adt_ensure_unique_consume, lumia_adt_ensure_unique_consume_mask,
    lumia_adt_ensure_unique_mask, lumia_adt_field, lumia_adt_set_field, lumia_adt_tag, lumia_cmp,
    lumia_hash,
};
pub use list::{
    lumia_ensure_list_f64, lumia_list_append, lumia_list_concat, lumia_list_empty, lumia_list_get,
    lumia_list_join, lumia_list_len, lumia_list_par_fold, lumia_list_par_map, lumia_list_promote,
    lumia_list_release, lumia_list_retain, lumia_list_reverse, lumia_list_set, lumia_list_slice,
    lumia_list_sort, lumia_list_sort_by_keys, lumia_list_take, lumia_ptr_eq, lumia_range,
    lumia_range_inclusive,
};
pub use map_set::{
    lumia_ensure_map_f64, lumia_ensure_map_vf64, lumia_ensure_set_f64, lumia_map_contains,
    lumia_map_finish, lumia_map_get, lumia_map_items, lumia_map_keys, lumia_map_remove,
    lumia_map_set, lumia_map_values, lumia_set_contains, lumia_set_finish, lumia_set_insert,
    lumia_set_remove,
};
pub use memo::{
    lumia_memo_idx_hits, lumia_memo_idx_lookup, lumia_memo_idx_misses, lumia_memo_idx_reset,
    lumia_memo_idx_store, lumia_memo_l2_hits, lumia_memo_l2_lookup, lumia_memo_l2_misses,
    lumia_memo_l2_reset, lumia_memo_l2_store,
};
pub use number_theory::{
    lumia_affine1_rem_sum, lumia_divisor_sum, lumia_gcd_sum, lumia_matmul_affine_checksum,
    lumia_product_rem_sum,
};
pub use primes::lumia_count_primes;
pub use show::{
    lumia_adt_set_bool_mask, lumia_adt_set_float_mask, lumia_alloc_char, lumia_show,
    lumia_show_adt, lumia_show_adt_named, lumia_show_bool, lumia_show_float, lumia_show_list_adt,
    lumia_show_list_bool, lumia_show_map_bool, lumia_show_set_bool,
};
pub use string_io::{
    lumia_alloc_string, lumia_assert, lumia_cstr_to_string, lumia_match_fail, lumia_println_auto,
    lumia_println_bool, lumia_println_cstr, lumia_println_float, lumia_println_int,
    lumia_println_str, lumia_println_unit, lumia_read_stdin, lumia_str_byte_len, lumia_str_concat, lumia_str_contains,
    lumia_str_ends_with, lumia_str_len, lumia_str_reverse, lumia_str_slice, lumia_str_split,
    lumia_str_starts_with, lumia_str_substring, lumia_str_take, lumia_str_to_lower,
    lumia_str_to_upper, lumia_str_trim, lumia_string_cstr, lumia_trap_div0, lumia_trap_overflow,
};
pub use task::{
    lumia_abi_handoff_set, lumia_channel_close, lumia_channel_new, lumia_channel_recv,
    lumia_channel_recv_opt, lumia_channel_send, lumia_scheduler_drain, lumia_scheduler_kind,
    lumia_scope_cancel, lumia_scope_enter, lumia_scope_leave, lumia_task_join, lumia_task_join_opt,
    lumia_task_spawn, lumia_task_spawn_nullary, SCHEDULER_IO, SCHEDULER_WORKER,
};

/// Push a Lumia frame name (nul-terminated) for trap backtraces.
///
/// # Safety
/// `name` must be null or a valid NUL-terminated C string that outlives the frame.
#[no_mangle]
pub unsafe extern "C" fn lumia_frame_push(name: *const u8) {
    common::frame_push(name);
}

/// Pop the top Lumia frame (pair with [`lumia_frame_push`]).
#[no_mangle]
pub extern "C" fn lumia_frame_pop() {
    common::frame_pop();
}

#[cfg(test)]
mod crate_tests;

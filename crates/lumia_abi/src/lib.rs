//! Shared ABI constants for the Lumia runtime and compiler.
//!
//! Codegen emits these `type_id` values into object headers; `lumia_rt` interprets
//! them. Memo caps must match between the opt planner and the runtime tables.
//!
//! This crate is intentionally a **shared kitchen sink** until further splits:
//! object `TYPE_*` / tid packing, memo + opt thresholds (`SPECIALIZE_CONST_*`,
//! `INLINE_*`), scheduler kind ints, and dense_f64 trampoline symbol tables.
//! Prefer adding a named section here over a third copy in codegen/opt.
//!
//! Float / container tagging rules: [`float_contract`].
//!
//! # Container `type_id` packing
//!
//! Bases occupy bits `[7:0]` (dense 1..=9). Float / AssocList flags live in
//! bits `[10:8]` so List/Map/Set no longer need a combinatorial ID matrix:
//!
//! - bit 8 `TID_F_KEY` — List: float elems; Set: float elems; Map: float keys
//! - bit 9 `TID_F_VAL` — Map: float values
//! - bit 10 `TID_ASSOC` — Map/Set: AssocList (never hash-promote)
//! - bit 11 `TID_HASH` — Map/Set: open-addressing hash table (vs linear payload)

mod float_contract;
pub use float_contract::{
    float_roles, gc_skip_float_slot, is_float_capable_container, FloatRoles, ENSURE_LIST_F64,
    ENSURE_MAP_F64, ENSURE_MAP_VF64, ENSURE_SET_F64,
};

/// Object header type ids — **bases** (bits `[7:0]`).
pub const TYPE_BYTES: u32 = 1;
pub const TYPE_STRING: u32 = 2;
pub const TYPE_LIST: u32 = 3;
pub const TYPE_MAP: u32 = 4;
pub const TYPE_SET: u32 = 5;
pub const TYPE_ADT: u32 = 6;
pub const TYPE_CHAR: u32 = 7;
/// Heap closure: `[fn_ptr:i64][cap0:i64]…`
pub const TYPE_CLOSURE: u32 = 8;
/// Virtual Int range list: payload `[start:i64][end_exclusive:i64]` (DESIGN §3.5 Iota).
pub const TYPE_LIST_IOTA: u32 = 9;
/// Task handle (effect concurrency; payload opaque to GC — sidecar in `lumia_rt`).
pub const TYPE_TASK: u32 = 10;
/// Channel handle (effect concurrency; buffer rooted via runtime sidecar).
pub const TYPE_CHANNEL: u32 = 11;

/// Object header byte size (`lumia_rt::ObjectHeader`); stack Lit* layouts use
/// [`OBJECT_HEADER_WORDS`] × i64 before payload.
pub const OBJECT_HEADER_BYTES: usize = 24;
pub const OBJECT_HEADER_WORDS: usize = 3;

/// Low bit set on FunRef i64 values so IndirectCall can distinguish them from
/// heap closures (pointer-aligned ⇒ bit 0 clear).
pub const FUNREF_TAG: i64 = 1;

/// Runtime trait dictionary ids (`lumia_dict_register` / codegen registration).
pub const TRAIT_SHOW: i32 = 1;
pub const TRAIT_EQ: i32 = 2;
pub const TRAIT_ORD: i32 = 3;
pub const TRAIT_HASH: i32 = 4;
pub const TRAIT_NUM: i32 = 5;

/// Mask / flags for packed container `type_id`s.
pub const TID_BASE_MASK: u32 = 0xFF;
/// List elems / Set elems / Map keys are unboxed Float bits (IEEE eq/hash).
pub const TID_F_KEY: u32 = 1 << 8;
/// Map values are unboxed Float bits.
pub const TID_F_VAL: u32 = 1 << 9;
/// Map/Set without Hash — linear forever (DESIGN AssocList).
pub const TID_ASSOC: u32 = 1 << 10;
/// Map/Set open-addressing hash table (vs small linear payload).
pub const TID_HASH: u32 = 1 << 11;

/// ADT Show-kind occupies bits `[31:16]` (0 = anonymous / `#tag` fallback).
pub const TID_ADT_KIND_SHIFT: u32 = 16;
pub const TID_ADT_KIND_MASK: u32 = 0xFFFF << TID_ADT_KIND_SHIFT;

/// Historical names as packed aliases (prefer constructors / flag helpers).
pub const TYPE_LIST_F64: u32 = TYPE_LIST | TID_F_KEY;
pub const TYPE_MAP_F64: u32 = TYPE_MAP | TID_F_KEY;
pub const TYPE_SET_F64: u32 = TYPE_SET | TID_F_KEY;
pub const TYPE_MAP_ASSOC: u32 = TYPE_MAP | TID_ASSOC;
pub const TYPE_SET_ASSOC: u32 = TYPE_SET | TID_ASSOC;
pub const TYPE_MAP_VF64: u32 = TYPE_MAP | TID_F_VAL;
pub const TYPE_MAP_F64V: u32 = TYPE_MAP | TID_F_KEY | TID_F_VAL;
pub const TYPE_MAP_ASSOC_VF64: u32 = TYPE_MAP | TID_ASSOC | TID_F_VAL;
pub const TYPE_MAP_ASSOC_F64: u32 = TYPE_MAP | TID_ASSOC | TID_F_KEY;
pub const TYPE_MAP_ASSOC_F64V: u32 = TYPE_MAP | TID_ASSOC | TID_F_KEY | TID_F_VAL;

/// Transparent memo (`T_f`) hard caps — must stay in sync across opt planner and rt.
///
/// Naming: DESIGN / Rust use `T_f` / `MEMO_TF_*`. Exported C symbols stay
/// `lumia_memo_l2_*` — **`L2` is a historical ABI name, frozen; do not rename.**
pub const MEMO_TF_MAX_FUNS: usize = 64;
pub const MEMO_TF_SLOTS: usize = 4;
pub const MEMO_TF_MAX_ARGS: usize = 4;
pub const MEMO_PROCESS_BYTE_CAP: usize = 2 * 1024 * 1024;
pub const MEMO_IDX_MAX_FUNS: usize = 16;
/// Keys outside `0..MEMO_IDX_CAP` are never cached (DESIGN §7.5 hard bound).
pub const MEMO_IDX_CAP: usize = 4096;
pub const MEMO_IDX_TABLE_BYTES: usize = MEMO_IDX_CAP * (1 + 8);
pub const MEMO_SLOTS_TABLE_BYTES: usize = MEMO_TF_SLOTS * (1 + MEMO_TF_MAX_ARGS * 8 + 8);

/// Max elems / key–value pairs for Lit\* / Small\* stack layouts and linear
/// Map·Set before hash promote. Escape analysis, ReprSelect, and RT must share
/// this threshold (DESIGN §7 representation selection).
pub const SMALL_CONTAINER_MAX: usize = 8;

/// Call-site const specialization caps (`SpecializeConstPass`).
pub const SPECIALIZE_CONST_MAX_CLONES_PER_FUN: usize = 16;
pub const SPECIALIZE_CONST_MAX_TOTAL_CLONES: usize = 64;
pub const SPECIALIZE_CONST_MAX_OPS: usize = 256;
/// Max nested inline expansions (`InlinePass`).
pub const INLINE_MAX_EXPAND_DEPTH: usize = 8;

/// RT: write per-field Float mask into ADT header `_pad` (after fields are live).
pub const ADT_SET_FLOAT_MASK: &str = "lumia_adt_set_float_mask";

/// Dense `List[Float]` kernels that opt may rewrite whole helpers into and that
/// codegen may emit as frameless trampolines.
///
/// Must stay ⊆ `runtime_decls` / `lumia_rt` exports. Scalar helpers (`sqrt` /
/// `exp` / …) and `checksum` are declared separately and are **not** trampoline
/// eligible.
pub const DENSE_F64_TRAMPOLINE_SYMS: &[&str] = &[
    "lumia_f64_gemv",
    "lumia_f64_gemv_t",
    "lumia_f64_addmm",
    "lumia_f64_axpy",
    "lumia_f64_sub",
    "lumia_f64_add",
    "lumia_f64_mul",
    "lumia_f64_clamp",
    "lumia_f64_scale",
    "lumia_f64_fill",
    "lumia_f64_copy",
    "lumia_list_f64_zeros",
    "lumia_f64_sum_sq",
    "lumia_f64_mean",
    "lumia_f64_std",
    "lumia_f64_l2_norm",
    "lumia_f64_softmax",
    "lumia_f64_l2_normalize",
];

#[inline]
pub fn is_dense_f64_trampoline(sym: &str) -> bool {
    DENSE_F64_TRAMPOLINE_SYMS.iter().any(|&s| s == sym)
}

/// Scheduler kind ids for `scope` / `ScopeEnter` (HIR Int / RT ABI).
///
/// `0` (and any other value) means the default cooperative scheduler.
pub const SCHEDULER_WORKER: i64 = 1;
pub const SCHEDULER_IO: i64 = 2;

/// Scalar classification for container element/key tagging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    Int,
    Float,
}

#[inline]
pub fn tid_base(tid: u32) -> u32 {
    tid & TID_BASE_MASK
}

#[inline]
pub fn tid_f_key(tid: u32) -> bool {
    tid & TID_F_KEY != 0
}

#[inline]
pub fn tid_f_val(tid: u32) -> bool {
    tid & TID_F_VAL != 0
}

#[inline]
pub fn tid_assoc(tid: u32) -> bool {
    tid & TID_ASSOC != 0
}

#[inline]
pub fn tid_hash(tid: u32) -> bool {
    tid & TID_HASH != 0
}

/// OR `TID_HASH` onto a packed Map/Set `type_id` (hash-table promote).
#[inline]
pub fn tid_with_hash(tid: u32) -> u32 {
    tid | TID_HASH
}

/// Clear `TID_HASH` when demoting a hash table back to a linear payload.
#[inline]
pub fn tid_without_hash(tid: u32) -> u32 {
    tid & !TID_HASH
}

/// OR `TID_F_KEY` onto an existing packed container `type_id` (empty-shell retag).
#[inline]
pub fn tid_with_f_key(tid: u32) -> u32 {
    tid | TID_F_KEY
}

/// OR `TID_F_VAL` onto an existing packed map `type_id` (empty-shell retag).
#[inline]
pub fn tid_with_f_val(tid: u32) -> u32 {
    tid | TID_F_VAL
}

/// Pack ADT base with a Show-kind id (`0` = no registered names).
#[inline]
pub fn adt_type_id(show_kind: u16) -> u32 {
    TYPE_ADT | ((show_kind as u32) << TID_ADT_KIND_SHIFT)
}

/// Extract Show-kind from an ADT `type_id` (`0` if unset / not an ADT).
#[inline]
pub fn adt_show_kind(tid: u32) -> u16 {
    if tid_base(tid) != TYPE_ADT {
        return 0;
    }
    ((tid & TID_ADT_KIND_MASK) >> TID_ADT_KIND_SHIFT) as u16
}

/// Heap list `type_id` from element scalar kind.
pub fn list_type_id(elem_is_float: bool) -> u32 {
    TYPE_LIST | if elem_is_float { TID_F_KEY } else { 0 }
}

/// Heap set `type_id` from element scalar kind and Hash availability.
pub fn set_type_id(elem_is_float: bool, assoc: bool) -> u32 {
    TYPE_SET | if elem_is_float { TID_F_KEY } else { 0 } | if assoc { TID_ASSOC } else { 0 }
}

/// Heap map `type_id` from key/value scalar kinds and Hash availability.
pub fn map_type_id(key_is_float: bool, val_is_float: bool, assoc: bool) -> u32 {
    TYPE_MAP
        | if key_is_float { TID_F_KEY } else { 0 }
        | if val_is_float { TID_F_VAL } else { 0 }
        | if assoc { TID_ASSOC } else { 0 }
}

/// True if `tid` is any heap Map representation.
#[inline]
pub fn is_map_tid(tid: u32) -> bool {
    tid_base(tid) == TYPE_MAP
}

/// True if `tid` is any heap Set representation.
#[inline]
pub fn is_set_tid(tid: u32) -> bool {
    tid_base(tid) == TYPE_SET
}

/// True if `tid` is a list (dense or Float-tagged or iota).
#[inline]
pub fn is_list_tid(tid: u32) -> bool {
    matches!(tid_base(tid), TYPE_LIST | TYPE_LIST_IOTA)
}

#[inline]
pub fn map_key_is_float(tid: u32) -> bool {
    is_map_tid(tid) && tid_f_key(tid)
}

#[inline]
pub fn map_val_is_float(tid: u32) -> bool {
    is_map_tid(tid) && tid_f_val(tid)
}

#[inline]
pub fn map_tid_is_assoc(tid: u32) -> bool {
    is_map_tid(tid) && tid_assoc(tid)
}

#[inline]
pub fn set_elem_is_float(tid: u32) -> bool {
    is_set_tid(tid) && tid_f_key(tid)
}

#[inline]
pub fn set_tid_is_assoc(tid: u32) -> bool {
    is_set_tid(tid) && tid_assoc(tid)
}

/// List elems are unboxed Float bits.
#[inline]
pub fn list_elem_is_float(tid: u32) -> bool {
    tid_base(tid) == TYPE_LIST && tid_f_key(tid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_bases_are_dense_and_unique() {
        let ids = [
            TYPE_BYTES,
            TYPE_STRING,
            TYPE_LIST,
            TYPE_MAP,
            TYPE_SET,
            TYPE_ADT,
            TYPE_CHAR,
            TYPE_CLOSURE,
            TYPE_LIST_IOTA,
            TYPE_TASK,
            TYPE_CHANNEL,
        ];
        let mut sorted = ids;
        sorted.sort_unstable();
        for w in sorted.windows(2) {
            assert_ne!(w[0], w[1], "duplicate type base");
        }
        assert_eq!(sorted[0], 1);
        assert_eq!(*sorted.last().unwrap(), TYPE_CHANNEL);
        assert_eq!(TID_F_KEY & TID_BASE_MASK, 0);
        assert_eq!(TID_F_VAL & TID_BASE_MASK, 0);
        assert_eq!(TID_ASSOC & TID_BASE_MASK, 0);
        assert_eq!(TID_HASH & TID_BASE_MASK, 0);
        assert_eq!(TID_HASH & (TID_F_KEY | TID_F_VAL | TID_ASSOC), 0);
        assert_eq!(SCHEDULER_WORKER, 1);
        assert_eq!(SCHEDULER_IO, 2);
        assert!(tid_hash(tid_with_hash(TYPE_MAP)));
        assert!(!tid_hash(TYPE_MAP));
        assert!(!tid_hash(tid_without_hash(tid_with_hash(TYPE_SET))));
        assert_eq!(tid_without_hash(tid_with_hash(TYPE_MAP)), TYPE_MAP);
        assert_eq!(adt_type_id(0), TYPE_ADT);
        assert_eq!(adt_show_kind(adt_type_id(0)), 0);
        assert_eq!(adt_show_kind(adt_type_id(42)), 42);
        assert_eq!(tid_base(adt_type_id(42)), TYPE_ADT);
        assert_eq!(TID_ADT_KIND_MASK & TID_BASE_MASK, 0);
    }

    #[test]
    fn map_type_id_grid() {
        assert_eq!(map_type_id(false, false, false), TYPE_MAP);
        assert_eq!(map_type_id(false, false, true), TYPE_MAP_ASSOC);
        assert_eq!(map_type_id(false, true, false), TYPE_MAP_VF64);
        assert_eq!(map_type_id(false, true, true), TYPE_MAP_ASSOC_VF64);
        assert_eq!(map_type_id(true, false, false), TYPE_MAP_F64);
        assert_eq!(map_type_id(true, false, true), TYPE_MAP_ASSOC_F64);
        assert_eq!(map_type_id(true, true, false), TYPE_MAP_F64V);
        assert_eq!(map_type_id(true, true, true), TYPE_MAP_ASSOC_F64V);
    }

    #[test]
    fn list_and_set_type_ids() {
        assert_eq!(list_type_id(false), TYPE_LIST);
        assert_eq!(list_type_id(true), TYPE_LIST_F64);
        assert_eq!(set_type_id(false, false), TYPE_SET);
        assert_eq!(set_type_id(false, true), TYPE_SET_ASSOC);
        assert_eq!(set_type_id(true, false), TYPE_SET_F64);
        // Packed: Float + Assoc coexist (old matrix dropped Assoc when Float).
        assert_eq!(set_type_id(true, true), TYPE_SET | TID_F_KEY | TID_ASSOC);
    }

    #[test]
    fn classifiers_agree_with_constructors() {
        for key in [false, true] {
            for val in [false, true] {
                for assoc in [false, true] {
                    let tid = map_type_id(key, val, assoc);
                    assert!(is_map_tid(tid));
                    assert_eq!(map_key_is_float(tid), key);
                    assert_eq!(map_val_is_float(tid), val);
                    assert_eq!(map_tid_is_assoc(tid), assoc);
                    assert_eq!(tid_base(tid), TYPE_MAP);
                }
            }
        }
        assert!(is_set_tid(TYPE_SET) && is_set_tid(TYPE_SET_F64) && is_set_tid(TYPE_SET_ASSOC));
        assert!(is_set_tid(set_type_id(true, true)));
        assert!(
            is_list_tid(TYPE_LIST) && is_list_tid(TYPE_LIST_F64) && is_list_tid(TYPE_LIST_IOTA)
        );
        assert!(!is_map_tid(TYPE_LIST) && !is_set_tid(TYPE_MAP));
        assert!(list_elem_is_float(TYPE_LIST_F64));
        assert!(!list_elem_is_float(TYPE_LIST));
        assert!(!list_elem_is_float(TYPE_LIST_IOTA));
    }

    #[test]
    fn packed_aliases_match_flags() {
        assert_eq!(TYPE_LIST_F64, TYPE_LIST | TID_F_KEY);
        assert_eq!(TYPE_MAP_F64V, TYPE_MAP | TID_F_KEY | TID_F_VAL);
        assert_eq!(
            TYPE_MAP_ASSOC_F64V,
            TYPE_MAP | TID_ASSOC | TID_F_KEY | TID_F_VAL
        );
    }

    #[test]
    fn tid_with_f_flags_or_onto_base() {
        assert_eq!(tid_with_f_key(TYPE_MAP), TYPE_MAP_F64);
        assert_eq!(tid_with_f_val(TYPE_MAP), TYPE_MAP_VF64);
        assert_eq!(tid_with_f_key(TYPE_MAP_ASSOC), TYPE_MAP_ASSOC_F64);
        assert_eq!(tid_with_f_key(TYPE_SET), TYPE_SET_F64);
        assert_eq!(tid_with_f_key(TYPE_LIST), TYPE_LIST_F64);
    }

    #[test]
    fn memo_caps_positive() {
        const {
            assert!(MEMO_TF_MAX_FUNS > 0);
            assert!(MEMO_TF_SLOTS > 0);
            assert!(MEMO_SLOTS_TABLE_BYTES > 0);
            assert!(MEMO_PROCESS_BYTE_CAP >= MEMO_IDX_TABLE_BYTES);
        }
        assert_eq!(MEMO_TF_MAX_ARGS, 4);
        assert_eq!(MEMO_IDX_TABLE_BYTES, MEMO_IDX_CAP * 9);
    }
}

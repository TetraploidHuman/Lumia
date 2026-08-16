//! Shared ABI constants for the Lumia runtime and compiler.
//!
//! Codegen emits these `type_id` values into object headers; `lumia_rt` interprets
//! them. Memo caps must match between the opt planner and the runtime tables.
//!
//! Modules:
//! - [`type_id`] — object `TYPE_*` / tid packing / classifiers
//! - [`memo`] — `MEMO_TF_*` / `SMALL_CONTAINER_MAX`
//! - [`opt_caps`] — specialize / inline thresholds
//! - [`dense_f64`] — trampoline symbol table
//! - [`scheduler`] — `scope` scheduler kind ints
//! - [`float_contract`] — float / container tagging rules
//!
//! Prefer extending the matching module over a third copy in codegen/opt.
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

mod dense_f64;
mod float_contract;
mod memo;
mod opt_caps;
mod scheduler;
mod type_id;

pub use dense_f64::{is_dense_f64_trampoline, DENSE_F64_TRAMPOLINE_SYMS};
pub use float_contract::{
    float_roles, gc_skip_float_slot, is_float_capable_container, FloatRoles, ENSURE_LIST_F64,
    ENSURE_MAP_F64, ENSURE_MAP_VF64, ENSURE_SET_F64,
};
pub use memo::{
    MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS, MEMO_IDX_TABLE_BYTES, MEMO_PROCESS_BYTE_CAP,
    MEMO_SLOTS_TABLE_BYTES, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS, SMALL_CONTAINER_MAX,
};
pub use opt_caps::{
    INLINE_MAX_EXPAND_DEPTH, SPECIALIZE_CONST_MAX_CLONES_PER_FUN, SPECIALIZE_CONST_MAX_OPS,
    SPECIALIZE_CONST_MAX_TOTAL_CLONES,
};
pub use scheduler::{SCHEDULER_IO, SCHEDULER_WORKER};
pub use type_id::{
    adt_show_kind, adt_type_id, is_list_tid, is_map_tid, is_set_tid, list_elem_is_float,
    list_type_id, map_key_is_float, map_tid_is_assoc, map_type_id, map_val_is_float,
    set_elem_is_float, set_tid_is_assoc, set_type_id, tid_assoc, tid_base, tid_f_key, tid_f_val,
    tid_hash, tid_with_f_key, tid_with_f_val, tid_with_hash, tid_without_hash, ScalarKind,
    ADT_SET_FLOAT_MASK, FUNREF_TAG, OBJECT_HEADER_BYTES, OBJECT_HEADER_WORDS, TID_ADT_KIND_MASK,
    TID_ADT_KIND_SHIFT, TID_ASSOC, TID_BASE_MASK, TID_F_KEY, TID_F_VAL, TID_HASH, TRAIT_EQ,
    TRAIT_HASH, TRAIT_NUM, TRAIT_ORD, TRAIT_SHOW, TYPE_ADT, TYPE_BYTES, TYPE_CHANNEL, TYPE_CHAR,
    TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA, TYPE_MAP, TYPE_MAP_ASSOC,
    TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64, TYPE_MAP_F64V,
    TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC, TYPE_SET_F64, TYPE_STRING, TYPE_TASK,
};

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

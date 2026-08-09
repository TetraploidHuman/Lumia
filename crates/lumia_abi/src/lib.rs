//! Shared ABI constants for the Lumia runtime and compiler.
//!
//! Codegen emits these `type_id` values into object headers; `lumia_rt` interprets
//! them. Memo caps must match between the opt planner and the runtime tables.

/// Object header type ids (descriptor table later).
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
/// Map/Set whose keys/elements are unboxed Float bits; eq/hash use IEEE (DESIGN §2.1).
pub const TYPE_MAP_F64: u32 = 10;
pub const TYPE_SET_F64: u32 = 11;
/// Map/Set without Hash — linear forever (DESIGN AssocList).
pub const TYPE_MAP_ASSOC: u32 = 12;
pub const TYPE_SET_ASSOC: u32 = 13;
/// List of unboxed Float bits; structural `==` / hash use IEEE (DESIGN §2.1).
pub const TYPE_LIST_F64: u32 = 14;
/// Map with Float values (Int/ADT keys); value `==` uses IEEE.
pub const TYPE_MAP_VF64: u32 = 15;
/// Map with Float keys and Float values.
pub const TYPE_MAP_F64V: u32 = 16;
/// AssocList + IEEE Float values (no Hash promotion).
pub const TYPE_MAP_ASSOC_VF64: u32 = 17;
/// AssocList + IEEE Float keys.
pub const TYPE_MAP_ASSOC_F64: u32 = 18;
/// AssocList + Float keys and Float values.
pub const TYPE_MAP_ASSOC_F64V: u32 = 19;

/// Transparent memo (`T_f`) hard caps — must stay in sync across opt planner and rt.
pub const MEMO_L2_MAX_FUNS: usize = 64;
pub const MEMO_L2_SLOTS: usize = 4;
pub const MEMO_L2_MAX_ARGS: usize = 4;
pub const MEMO_PROCESS_BYTE_CAP: usize = 2 * 1024 * 1024;
pub const MEMO_IDX_MAX_FUNS: usize = 16;
/// Keys outside `0..MEMO_IDX_CAP` are never cached (DESIGN §7.5 hard bound).
pub const MEMO_IDX_CAP: usize = 4096;
pub const MEMO_IDX_TABLE_BYTES: usize = MEMO_IDX_CAP * (1 + 8);
pub const MEMO_SLOTS_TABLE_BYTES: usize = MEMO_L2_SLOTS * (1 + MEMO_L2_MAX_ARGS * 8 + 8);

/// Scalar classification for container element/key tagging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    Int,
    Float,
}

/// Heap list `type_id` from element scalar kind.
pub fn list_type_id(elem_is_float: bool) -> u32 {
    if elem_is_float {
        TYPE_LIST_F64
    } else {
        TYPE_LIST
    }
}

/// Heap set `type_id` from element scalar kind and Hash availability.
pub fn set_type_id(elem_is_float: bool, assoc: bool) -> u32 {
    if elem_is_float {
        TYPE_SET_F64
    } else if assoc {
        TYPE_SET_ASSOC
    } else {
        TYPE_SET
    }
}

/// Heap map `type_id` from key/value scalar kinds and Hash availability.
///
/// Float-value tags win over Assoc for IEEE value `==`; Assoc is for key Hash
/// absence (linear forever) when values are not Float.
pub fn map_type_id(key_is_float: bool, val_is_float: bool, assoc: bool) -> u32 {
    match (key_is_float, val_is_float, assoc) {
        (true, true, true) => TYPE_MAP_ASSOC_F64V,
        (true, false, true) => TYPE_MAP_ASSOC_F64,
        (false, true, true) => TYPE_MAP_ASSOC_VF64,
        (true, true, false) => TYPE_MAP_F64V,
        (true, false, false) => TYPE_MAP_F64,
        (false, true, false) => TYPE_MAP_VF64,
        (false, false, true) => TYPE_MAP_ASSOC,
        (false, false, false) => TYPE_MAP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_ids_are_dense_and_unique() {
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
            TYPE_MAP_F64,
            TYPE_SET_F64,
            TYPE_MAP_ASSOC,
            TYPE_SET_ASSOC,
            TYPE_LIST_F64,
            TYPE_MAP_VF64,
            TYPE_MAP_F64V,
            TYPE_MAP_ASSOC_VF64,
            TYPE_MAP_ASSOC_F64,
            TYPE_MAP_ASSOC_F64V,
        ];
        let mut sorted = ids;
        sorted.sort_unstable();
        for w in sorted.windows(2) {
            assert_ne!(w[0], w[1], "duplicate type_id");
        }
        assert_eq!(sorted[0], 1);
        assert_eq!(*sorted.last().unwrap(), TYPE_MAP_ASSOC_F64V);
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
        assert_eq!(set_type_id(true, true), TYPE_SET_F64);
    }

    #[test]
    fn memo_caps_positive() {
        assert!(MEMO_L2_MAX_FUNS > 0);
        assert!(MEMO_L2_SLOTS > 0);
        assert_eq!(MEMO_L2_MAX_ARGS, 4);
        assert_eq!(MEMO_IDX_TABLE_BYTES, MEMO_IDX_CAP * 9);
        assert!(MEMO_SLOTS_TABLE_BYTES > 0);
        assert!(MEMO_PROCESS_BYTE_CAP >= MEMO_IDX_TABLE_BYTES);
    }
}

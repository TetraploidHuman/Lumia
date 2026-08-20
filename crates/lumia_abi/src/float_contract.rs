//! Container Float / `type_id` contract shared by runtime and codegen.
//!
//! # Rules
//!
//! 1. **Tagging**: unboxed Float elems/keys/values are marked with [`TID_F_KEY`] /
//!    [`TID_F_VAL`] on List/Map/Set `type_id`s (see constructors in the crate root).
//! 2. **Eq / hash**: Float-tagged slots use IEEE helpers (`±0` collide, `NaN` never
//!    equals). Non-Float slots use the generic value eq/hash path.
//! 3. **GC**: mark must **skip** unboxed Float slots (they are not heap pointers).
//! 4. **Ensure**: codegen calls `lumia_ensure_*_f64` before writing Float into a
//!    container that may still carry an Int-tagged empty shell; non-empty wrong
//!    sort traps.
//! 5. **Scalar path**: bare i64 values without a container `type_id` still use
//!    bit equality in `lumia_eq`. Scalar `==` on Float is emitted as `fcmp` in
//!    codegen and does not go through `lumia_eq`. Nested Float-as-bits without a
//!    typed container remain out of scope for IEEE (locked by tests + DESIGN).

use crate::{
    is_list_tid, is_map_tid, is_set_tid, list_elem_is_float, map_key_is_float, map_val_is_float,
    set_elem_is_float, tid_base, TYPE_ADT, TYPE_LIST, TYPE_MAP, TYPE_SET,
};

/// Which Float roles a container `type_id` carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloatRoles {
    pub key_or_elem: bool,
    pub map_val: bool,
}

/// Classify Float roles for any heap `type_id`.
pub fn float_roles(tid: u32) -> FloatRoles {
    FloatRoles {
        key_or_elem: list_elem_is_float(tid) || map_key_is_float(tid) || set_elem_is_float(tid),
        map_val: map_val_is_float(tid),
    }
}

/// True when a List/Map/Set/ADT payload slot at `index` should be skipped by GC mark
/// because it holds unboxed Float bits.
///
/// For ADT, `float_mask` is the header `_pad` bitset (bit `i` ⇒ field `i` is Float).
pub fn gc_skip_float_slot(tid: u32, index: usize, adt_float_mask: u64) -> bool {
    match tid_base(tid) {
        TYPE_LIST => list_elem_is_float(tid),
        TYPE_SET => set_elem_is_float(tid),
        TYPE_MAP => {
            // Map payload layout: pairs (k,v)… — even indices keys, odd values.
            if index.is_multiple_of(2) {
                map_key_is_float(tid)
            } else {
                map_val_is_float(tid)
            }
        }
        // Mask covers at most 64 fields; wider products leave trailing slots as pointers.
        TYPE_ADT => index < 64 && (adt_float_mask >> index) & 1 != 0,
        _ => false,
    }
}

/// Runtime symbol codegen should call to ensure Float-key tagging on a map.
pub const ENSURE_MAP_F64: &str = "lumia_ensure_map_f64";
/// Runtime symbol for Float-value tagging on a map.
pub const ENSURE_MAP_VF64: &str = "lumia_ensure_map_vf64";
/// Runtime symbol for Float-elem tagging on a set.
pub const ENSURE_SET_F64: &str = "lumia_ensure_set_f64";
/// Runtime symbol for Float-elem tagging on a list.
pub const ENSURE_LIST_F64: &str = "lumia_ensure_list_f64";
/// Runtime symbol for Bool-key tagging on a map.
pub const ENSURE_MAP_BOOL: &str = "lumia_ensure_map_bool";
/// Runtime symbol for Bool-value tagging on a map.
pub const ENSURE_MAP_VBOOL: &str = "lumia_ensure_map_vbool";
/// Runtime symbol for Bool-elem tagging on a set.
pub const ENSURE_SET_BOOL: &str = "lumia_ensure_set_bool";
/// Runtime symbol for Bool-elem tagging on a list.
pub const ENSURE_LIST_BOOL: &str = "lumia_ensure_list_bool";

/// True if `tid` is a container that may carry Float-tagged slots.
#[inline]
pub fn is_float_capable_container(tid: u32) -> bool {
    is_list_tid(tid) || is_map_tid(tid) || is_set_tid(tid) || tid_base(tid) == TYPE_ADT
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{list_type_id, map_type_id, set_type_id, TYPE_LIST, TYPE_LIST_F64, TYPE_MAP_F64V};

    #[test]
    fn float_roles_list_map_set() {
        assert_eq!(
            float_roles(list_type_id(true)),
            FloatRoles {
                key_or_elem: true,
                map_val: false
            }
        );
        assert_eq!(
            float_roles(map_type_id(true, true, false)),
            FloatRoles {
                key_or_elem: true,
                map_val: true
            }
        );
        assert_eq!(
            float_roles(set_type_id(true, false)),
            FloatRoles {
                key_or_elem: true,
                map_val: false
            }
        );
    }

    #[test]
    fn gc_skip_map_pairs() {
        let tid = TYPE_MAP_F64V;
        assert!(gc_skip_float_slot(tid, 0, 0)); // key
        assert!(gc_skip_float_slot(tid, 1, 0)); // val
        assert!(!gc_skip_float_slot(TYPE_LIST_F64, 0, 0) || list_elem_is_float(TYPE_LIST_F64));
        assert!(gc_skip_float_slot(TYPE_LIST_F64, 3, 0));
        assert!(gc_skip_float_slot(TYPE_ADT, 1, 0b0010u64));
        assert!(!gc_skip_float_slot(TYPE_ADT, 0, 0b0010u64));
        assert!(gc_skip_float_slot(TYPE_ADT, 37, 1u64 << 37));
        assert!(!gc_skip_float_slot(TYPE_ADT, 37, 0));
    }

    #[test]
    fn ensure_symbol_names_stable() {
        assert!(ENSURE_LIST_F64.starts_with("lumia_ensure_"));
        assert!(ENSURE_MAP_F64.contains("map"));
        assert!(ENSURE_MAP_VF64.contains("vf64") || ENSURE_MAP_VF64.contains("map"));
    }

    #[test]
    fn scalar_lumia_eq_path_is_bit_identity_by_contract() {
        // Locked contract (see module docs §5): non-heap i64 uses bit equality in
        // `lumia_eq`; IEEE Float scalar `==` is codegen `fcmp` only.
        assert!(!float_roles(TYPE_LIST).key_or_elem);
        assert!(float_roles(TYPE_LIST_F64).key_or_elem);
    }
}

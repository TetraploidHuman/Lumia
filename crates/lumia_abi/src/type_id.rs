//! Object header `type_id` bases, packing flags, and classifiers.

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

/// RT: write per-field Float mask into ADT header `_pad` (after fields are live).
pub const ADT_SET_FLOAT_MASK: &str = "lumia_adt_set_float_mask";

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

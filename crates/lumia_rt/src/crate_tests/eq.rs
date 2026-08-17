use super::*;
use crate::common::{list_rc_is_unique, list_rc_retain, RC_SHARED};

#[test]
fn object_header_pad_list_rc_vs_adt_float_mask() {
    // List: alloc initializes `rc` to 1 (unique); retain bumps RC.
    let list = lumia_alloc(8, TYPE_LIST);
    unsafe {
        *(list as *mut i64) = 0;
        assert_eq!((*header_from_payload(list)).rc, 1);
        assert_eq!((*header_from_payload(list))._pad, 0);
        assert!(list_rc_is_unique(list));
    }
    list_rc_retain(list);
    unsafe {
        assert_eq!((*header_from_payload(list)).rc, 2);
        assert!(!list_rc_is_unique(list));
    }

    // ADT: `_pad` stores float field mask; `rc` is independent COW count.
    let adt = lumia_alloc(16, TYPE_ADT); // tag + 1 field
    unsafe {
        *(adt as *mut i64) = 0;
        *((adt as *mut i64).add(1)) = 0.5f64.to_bits() as i64;
        assert_eq!((*header_from_payload(adt)).rc, 1);
        assert_eq!((*header_from_payload(adt))._pad, 0);
    }
    unsafe { lumia_adt_set_float_mask(adt, 0b1); }
    unsafe {
        assert_eq!((*header_from_payload(adt))._pad, 0b1);
        assert_eq!((*header_from_payload(adt)).rc, 1);
    }

    // Immortal empty-list singleton uses RC_SHARED in `rc`.
    let empty = lumia_list_empty();
    unsafe {
        assert_eq!((*header_from_payload(empty)).rc, RC_SHARED);
    }
}

#[test]
fn list_f64_eq_follows_ieee() {
    let pos0 = 0.0f64.to_bits() as i64;
    let neg0 = (-0.0f64).to_bits() as i64;
    let nan = f64::NAN.to_bits() as i64;
    let a = {
        let p = lumia_alloc(list_payload_bytes(1), lumia_abi::list_type_id(true));
        unsafe {
            *(p as *mut i64) = 1;
            *((p as *mut i64).add(1)) = pos0;
        }
        p
    };
    let b = {
        let p = lumia_alloc(list_payload_bytes(1), lumia_abi::list_type_id(true));
        unsafe {
            *(p as *mut i64) = 1;
            *((p as *mut i64).add(1)) = neg0;
        }
        p
    };
    let c = {
        let p = lumia_alloc(list_payload_bytes(1), lumia_abi::list_type_id(true));
        unsafe {
            *(p as *mut i64) = 1;
            *((p as *mut i64).add(1)) = nan;
        }
        p
    };
    assert_eq!(lumia_eq(a as i64, b as i64), 1);
    // Same object still NaN≠NaN under IEEE content compare.
    assert_eq!(lumia_eq(c as i64, c as i64), 0);
    let c2 = {
        let p = lumia_alloc(list_payload_bytes(1), lumia_abi::list_type_id(true));
        unsafe {
            *(p as *mut i64) = 1;
            *((p as *mut i64).add(1)) = nan;
        }
        p
    };
    assert_eq!(lumia_eq(c as i64, c2 as i64), 0);
}

#[test]
fn adt_float_mask_nested_eq_and_hash() {
    let pos0 = 0.0f64.to_bits() as i64;
    let neg0 = (-0.0f64).to_bits() as i64;
    let mk = |bits: i64| {
        let p = lumia_alloc(16, TYPE_ADT); // tag + 1 field
        unsafe {
            *(p as *mut i64) = 0; // tag Some
            *((p as *mut i64).add(1)) = bits;
        }
        unsafe { lumia_adt_set_float_mask(p, 1) }; // field0 is Float
        p as i64
    };
    let a = mk(pos0);
    let b = mk(neg0);
    assert_eq!(lumia_eq(a, b), 1);
    assert_eq!(lumia_hash(a), lumia_hash(b));
    // List of ADTs also compares via stored masks.
    let la = {
        let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST);
        unsafe {
            *(p as *mut i64) = 1;
            *((p as *mut i64).add(1)) = a;
        }
        p as i64
    };
    let lb = {
        let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST);
        unsafe {
            *(p as *mut i64) = 1;
            *((p as *mut i64).add(1)) = b;
        }
        p as i64
    };
    assert_eq!(lumia_eq(la, lb), 1);
}

#[test]
fn adt_float_mask_high_bit_skips_gc_mark() {
    // Field index 31 (top of packed float half) must be skippable as Float.
    let bits = 1.25f64.to_bits() as i64;
    let nbytes = (1 + 32) * 8;
    let mut adt = lumia_alloc(nbytes, TYPE_ADT);
    unsafe {
        let base = adt as *mut i64;
        *base = 0;
        for i in 1..=31 {
            *base.add(i) = 0;
        }
        *base.add(32) = bits;
    }
    unsafe { lumia_adt_set_float_mask(adt, 1u64 << 31); }
    unsafe { lumia_root_push(&mut adt as *mut *mut u8) };
    lumia_gc_collect();
    unsafe {
        assert_eq!((*(adt as *const i64).add(32)), bits);
        assert_eq!(
            crate::common::adt_float_mask((*header_from_payload(adt))._pad),
            1u64 << 31
        );
    }
    lumia_root_pop();
}

#[test]
fn adt_bool_mask_packs_high_half_preserves_float() {
    let adt = crate::map_set::alloc_adt_with_meta(0, &[1, 0], 0, 0, 0);
    unsafe { lumia_adt_set_float_mask(adt, 0b1); }
    unsafe { lumia_adt_set_bool_mask(adt, 0b10); }
    unsafe {
        let pad = (*header_from_payload(adt))._pad;
        assert_eq!(crate::common::adt_float_mask(pad), 0b1);
        assert_eq!(crate::common::adt_bool_mask(pad), 0b10);
        assert_eq!(
            pad,
            crate::common::adt_pack_field_masks(0b1, 0b10)
        );
    }
}

#[test]
fn adt_float_mask_sanitizes_heap_pointer_slots() {
    // Product mono may type a List field as Float; set_float_mask must clear that bit.
    let list = lumia_alloc(8, TYPE_LIST);
    unsafe {
        *(list as *mut i64) = 0;
    }
    let adt = lumia_alloc(16, TYPE_ADT); // tag + 1 field
    unsafe {
        *(adt as *mut i64) = 0;
        *((adt as *mut i64).add(1)) = list as i64;
    }
    unsafe { lumia_adt_set_float_mask(adt, 0b1); }
    unsafe {
        assert_eq!(
            (*header_from_payload(adt))._pad,
            0,
            "heap pointer slot must not stay Float-tagged"
        );
    }
}

#[test]
fn adt_mistagged_float_mask_skips_mark_trust_sanitize() {
    // `lumia_adt_set_float_mask` sanitizes heap-pointer slots. Bypass sanitize
    // (force `_pad`) is undefined for GC: Float-tagged words are not marked.
    // Production paths must not mistag; this locks the skip-without-probe contract.
    let list = lumia_alloc(8, TYPE_LIST);
    unsafe {
        *(list as *mut i64) = 0;
    }
    let list_bits = list as i64;
    let mut adt = lumia_alloc(16, TYPE_ADT);
    unsafe {
        *(adt as *mut i64) = 0;
        *((adt as *mut i64).add(1)) = list_bits;
        (*header_from_payload(adt))._pad = 0b1;
    }
    unsafe { lumia_root_push(&mut adt as *mut *mut u8) };
    lumia_gc_collect();
    assert!(
        !crate::common::is_heap_payload(list_bits as *mut u8),
        "mistagged Float slot is not marked (trust sanitize; do not bypass _pad)"
    );
    lumia_root_pop();
}

#[test]
fn adt_ensure_unique_consume_drops_with_alias() {
    // with-temp retain (rc=2) + consume → unique in-place.
    let adt = lumia_alloc(24, TYPE_ADT);
    unsafe {
        let b = adt as *mut i64;
        *b = 0;
        *b.add(1) = 1;
        *b.add(2) = 2;
        (*header_from_payload(adt)).rc = 2;
    }
    let out = unsafe { lumia_adt_ensure_unique_consume(adt) };
    assert_eq!(out, adt);
    unsafe {
        assert_eq!((*header_from_payload(adt)).rc, 1);
    }
    unsafe { lumia_adt_set_field(adt, 0, 9) };
    unsafe {
        assert_eq!(*(adt as *const i64).add(1), 9);
        assert_eq!(*(adt as *const i64).add(2), 2);
    }
}

#[test]
fn adt_ensure_unique_clones_when_shared() {
    let adt = lumia_alloc(24, TYPE_ADT);
    unsafe {
        let b = adt as *mut i64;
        *b = 0;
        *b.add(1) = 1;
        *b.add(2) = 2;
        (*header_from_payload(adt)).rc = 2;
    }
    let out = unsafe { lumia_adt_ensure_unique(adt) };
    assert_ne!(out as usize, adt as usize);
    unsafe {
        assert_eq!((*header_from_payload(out)).rc, 1);
        assert_eq!(*(out as *const i64).add(1), 1);
        assert_eq!(*(out as *const i64).add(2), 2);
        // Nested retain is a no-op for immediate int fields.
        assert_eq!((*header_from_payload(adt)).rc, 2);
    }
}

#[test]
fn adt_ensure_unique_consume_clones_when_shared() {
    let adt = lumia_alloc(24, TYPE_ADT);
    unsafe {
        let b = adt as *mut i64;
        *b = 0;
        *b.add(1) = 1;
        *b.add(2) = 2;
        (*header_from_payload(adt)).rc = 3;
    }
    let out = unsafe { lumia_adt_ensure_unique_consume(adt) };
    assert_ne!(out as usize, adt as usize);
    unsafe {
        assert_eq!((*header_from_payload(out)).rc, 1);
        assert_eq!(*(out as *const i64).add(1), 1);
        assert_eq!(*(out as *const i64).add(2), 2);
    }
}

#[test]
fn adt_set_field_retains_nested_adt() {
    let inner = lumia_alloc(16, TYPE_ADT);
    unsafe {
        *(inner as *mut i64) = 0;
        *((inner as *mut i64).add(1)) = 7;
        assert_eq!((*header_from_payload(inner)).rc, 1);
    }
    let outer = lumia_alloc(24, TYPE_ADT);
    unsafe {
        *(outer as *mut i64) = 0;
        *((outer as *mut i64).add(1)) = 0;
        *((outer as *mut i64).add(2)) = 0;
    }
    unsafe { lumia_adt_set_field(outer, 0, inner as i64) };
    unsafe {
        assert_eq!((*header_from_payload(inner)).rc, 2);
    }
    let inner2 = lumia_alloc(16, TYPE_ADT);
    unsafe {
        *(inner2 as *mut i64) = 0;
        *((inner2 as *mut i64).add(1)) = 8;
    }
    unsafe { lumia_adt_set_field(outer, 0, inner2 as i64) };
    unsafe {
        assert_eq!((*header_from_payload(inner)).rc, 1);
        assert_eq!((*header_from_payload(inner2)).rc, 2);
    }
}

#[test]
fn adt_clone_retains_nested_field() {
    let inner = lumia_alloc(16, TYPE_ADT);
    unsafe {
        *(inner as *mut i64) = 0;
        *((inner as *mut i64).add(1)) = 3;
    }
    let outer = lumia_alloc(24, TYPE_ADT);
    unsafe {
        *(outer as *mut i64) = 0;
        *((outer as *mut i64).add(1)) = inner as i64;
        *((outer as *mut i64).add(2)) = 1;
        // Simulate parent-field retain + extra alias.
        (*header_from_payload(inner)).rc = 2;
        (*header_from_payload(outer)).rc = 2;
    }
    let cloned = unsafe { lumia_adt_ensure_unique(outer) };
    assert_ne!(cloned as usize, outer as usize);
    unsafe {
        // Clone retains nested Inner once more.
        assert_eq!((*header_from_payload(inner)).rc, 3);
        assert_eq!(*((cloned as *const i64).add(1)), inner as i64);
    }
}

#[test]
fn adt_clone_overwrite_mask_skips_nested_retain() {
    let inner = lumia_alloc(16, TYPE_ADT);
    unsafe {
        *(inner as *mut i64) = 0;
        *((inner as *mut i64).add(1)) = 3;
        (*header_from_payload(inner)).rc = 2;
    }
    let outer = lumia_alloc(24, TYPE_ADT);
    unsafe {
        *(outer as *mut i64) = 0;
        *((outer as *mut i64).add(1)) = inner as i64;
        *((outer as *mut i64).add(2)) = 1;
        (*header_from_payload(outer)).rc = 2;
    }
    // Field 0 will be overwritten — do not bump inner RC on clone.
    let cloned = unsafe { lumia_adt_ensure_unique_mask(outer, 1) };
    assert_ne!(cloned as usize, outer as usize);
    unsafe {
        assert_eq!((*header_from_payload(inner)).rc, 2);
        assert_eq!(*((cloned as *const i64).add(1)), 0);
        assert_eq!(*((cloned as *const i64).add(2)), 1);
    }
    let replacement = lumia_alloc(16, TYPE_ADT);
    unsafe {
        *(replacement as *mut i64) = 0;
        *((replacement as *mut i64).add(1)) = 9;
    }
    unsafe { lumia_adt_set_field(cloned, 0, replacement as i64) };
    unsafe {
        assert_eq!((*header_from_payload(inner)).rc, 2);
        assert_eq!((*header_from_payload(replacement)).rc, 2);
        assert_eq!(*((cloned as *const i64).add(1)), replacement as i64);
    }
}

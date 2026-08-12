use super::*;
use crate::common::{list_rc_is_unique, list_rc_retain, RC_SHARED};

#[test]
fn object_header_pad_list_rc_vs_adt_float_mask() {
    // List: alloc initializes `_pad` to 1 (unique); retain bumps RC.
    let list = lumia_alloc(8, TYPE_LIST);
    unsafe {
        *(list as *mut i64) = 0;
        assert_eq!((*header_from_payload(list))._pad, 1);
        assert!(list_rc_is_unique(list));
    }
    list_rc_retain(list);
    unsafe {
        assert_eq!((*header_from_payload(list))._pad, 2);
        assert!(!list_rc_is_unique(list));
    }

    // ADT: `_pad` stores float field mask, not RC.
    let adt = lumia_alloc(16, TYPE_ADT);
    unsafe {
        assert_eq!((*header_from_payload(adt))._pad, 0);
    }
    lumia_adt_set_float_mask(adt, 0b101);
    unsafe {
        assert_eq!((*header_from_payload(adt))._pad, 0b101);
    }

    // Immortal empty-list singleton uses RC_SHARED in `_pad`.
    let empty = lumia_list_empty();
    unsafe {
        assert_eq!((*header_from_payload(empty))._pad, RC_SHARED);
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
        lumia_adt_set_float_mask(p, 1); // field0 is Float
        unsafe {
            *(p as *mut i64) = 0; // tag Some
            *((p as *mut i64).add(1)) = bits;
        }
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

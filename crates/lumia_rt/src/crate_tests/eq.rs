use super::*;

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

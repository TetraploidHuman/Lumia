//! Lumia runtime: pluggable GC ABI + first MmBackend (STW mark-sweep).
//!
//! C ABI contract used by codegen:
//! - `lumia_alloc(nbytes, type_id) -> *mut u8`
//! - `lumia_root_push(*mut *mut u8)` / `lumia_root_pop()`
//! - `lumia_write_barrier(obj, field_index, new_ptr)` — no-op under STW mark-sweep
//!   (roots are exact; concurrent/incremental collectors would use a real barrier)
//!   (precise shadow-stack roots; barrier is part of the stable MmBackend ABI and
//!   becomes meaningful for concurrent / generational collectors).
//! - `lumia_gc_collect()`
//! - `lumia_println_int(i64)` / `lumia_println_str(*const u8, len)`
//!
//! C ABI entry points take raw pointers by design; they are not Rust `unsafe fn`
//! because the LLVM-emitted caller already treats the boundary as unchecked.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod common;
mod dict;
mod gc;
mod list;
mod map_set;
mod memo;
mod show_eq;
mod string_io;

use common::{header_from_payload, trap_abort, GcInhibitGuard};
pub use common::{
    MarkSweep, MmBackend, ObjectHeader, MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS, MEMO_IDX_TABLE_BYTES,
    MEMO_L2_MAX_ARGS, MEMO_L2_MAX_FUNS, MEMO_L2_SLOTS, MEMO_PROCESS_BYTE_CAP, TYPE_ADT, TYPE_BYTES,
    TYPE_CHAR, TYPE_CLOSURE, TYPE_LIST, TYPE_LIST_F64, TYPE_LIST_IOTA, TYPE_MAP, TYPE_MAP_ASSOC,
    TYPE_MAP_ASSOC_F64, TYPE_MAP_ASSOC_F64V, TYPE_MAP_ASSOC_VF64, TYPE_MAP_F64, TYPE_MAP_F64V,
    TYPE_MAP_VF64, TYPE_SET, TYPE_SET_ASSOC, TYPE_SET_F64, TYPE_STRING,
};

use gc::list_payload_bytes;
pub use gc::{lumia_alloc, lumia_gc_collect, lumia_root_pop, lumia_root_push, lumia_write_barrier};

pub use dict::{
    lumia_dict_lookup, lumia_dict_register, lumia_dict_show, TRAIT_EQ, TRAIT_HASH, TRAIT_NUM,
    TRAIT_ORD, TRAIT_SHOW,
};
pub use list::*;
pub use map_set::*;
pub use memo::*;
pub use show_eq::*;
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

#[no_mangle]
pub extern "C" fn lumia_len(obj: *mut u8) -> i64 {
    if obj.is_null() {
        return 0;
    }
    unsafe {
        let h = header_from_payload(obj);
        match (*h).type_id {
            TYPE_STRING => (*h).size as i64,
            TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => list_len_of(obj),
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => *(obj as *const i64),
            tid if is_map_tid(tid) => map_count(obj),
            _ => trap_abort(&format!("lumia: len on unsupported type {}", (*h).type_id)),
        }
    }
}
#[no_mangle]
pub extern "C" fn lumia_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let ta = if a.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(a)).type_id }
    };
    let tb = if b.is_null() {
        ta
    } else {
        unsafe { (*header_from_payload(b)).type_id }
    };
    if ta == TYPE_STRING || tb == TYPE_STRING {
        if ta != TYPE_STRING || tb != TYPE_STRING {
            trap_abort("lumia: concat type mismatch");
        }
        return lumia_str_concat(a, b);
    }
    lumia_list_concat(a, b)
}
#[no_mangle]
pub extern "C" fn lumia_set(obj: *mut u8, key_or_index: i64, val: i64) -> *mut u8 {
    if obj.is_null() {
        return lumia_map_set(obj, key_or_index, val);
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => lumia_list_set(obj, key_or_index, val),
        tid if is_map_tid(tid) => lumia_map_set(obj, key_or_index, val),
        _ => trap_abort(&format!("lumia: set on unsupported type_id={tid}")),
    }
}
#[no_mangle]
pub extern "C" fn lumia_elems(obj: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if obj.is_null() {
        let dest = lumia_alloc(8, TYPE_LIST);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        TYPE_LIST | TYPE_LIST_F64 => obj,
        TYPE_LIST_IOTA => force_heap_list(obj),
        TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => unsafe {
            let n = *(obj as *const i64);
            let nbytes = list_payload_bytes(n);
            let dest = lumia_alloc(nbytes, TYPE_LIST);
            let dst = dest as *mut i64;
            *dst = n;
            for i in 0..n as usize {
                *dst.add(1 + i) = set_elem_at(obj, i);
            }
            dest
        },
        tid if is_map_tid(tid) => lumia_map_keys(obj),
        other => trap_abort(&format!("lumia: elems unsupported type_id={other}")),
    }
}
#[no_mangle]
pub extern "C" fn lumia_remove(obj: *mut u8, key_or_elem: i64) -> *mut u8 {
    if obj.is_null() {
        // Ambiguous empty — prefer Map (same historical default as typed `remove`).
        return lumia_map_remove(obj, key_or_elem);
    }
    let tid = unsafe { (*header_from_payload(obj)).type_id };
    match tid {
        tid if is_map_tid(tid) => lumia_map_remove(obj, key_or_elem),
        TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => lumia_set_remove(obj, key_or_elem),
        _ => trap_abort(&format!("lumia: remove on unsupported type_id={tid}")),
    }
}

/// Dispatch get: List/Set by index → i64 elem; Map by key → Option ADT ptr as i64.
#[no_mangle]
pub extern "C" fn lumia_get(obj: *mut u8, key_or_index: i64, some_tag: i64, none_tag: i64) -> i64 {
    if obj.is_null() {
        trap_abort("lumia: get on null");
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            TYPE_LIST | TYPE_LIST_F64 | TYPE_LIST_IOTA => lumia_list_get(obj, key_or_index),
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => {
                let n = *(obj as *const i64);
                if key_or_index < 0 || key_or_index >= n {
                    trap_abort("lumia: set get OOB");
                }
                set_elem_at(obj, key_or_index as usize)
            }
            tid if is_map_tid(tid) => {
                let opt = lumia_map_get(obj, key_or_index, some_tag, none_tag);
                opt as i64
            }
            other => trap_abort(&format!("lumia: get unsupported type_id {other}")),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_contains(obj: *mut u8, key: i64) -> i64 {
    if obj.is_null() {
        return 0;
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            tid if is_map_tid(tid) => lumia_map_contains(obj, key),
            TYPE_SET | TYPE_SET_F64 | TYPE_SET_ASSOC => lumia_set_contains(obj, key),
            TYPE_STRING => lumia_str_contains(obj, key as *mut u8),
            other => trap_abort(&format!("lumia: contains unsupported type_id {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{header_from_payload, trap_abort, PAR_WORKER};
    use crate::gc::list_payload_bytes;
    use crate::list::force_heap_list;
    use crate::map_set::{
        map_count, map_is_assoc, map_is_hash, map_is_overlay, map_overlay_dn, set_elem_at,
        set_is_hash,
    };
    use crate::string_io::with_str_bytes;
    use crate::MmBackend;
    use std::ptr;

    #[test]
    #[should_panic(expected = "stack trace:")]
    fn trap_prints_call_stack() {
        let a = b"alpha\0";
        let b = b"beta\0";
        lumia_frame_push(a.as_ptr());
        lumia_frame_push(b.as_ptr());
        trap_abort("lumia: test trap");
    }

    #[test]
    fn alloc_and_collect_unrooted() {
        let p = lumia_alloc(16, TYPE_BYTES);
        assert!(!p.is_null());
        // Not rooted → collect should free
        lumia_gc_collect();
        // Heap should be empty or not contain live unmarked — allocate again
        let q = lumia_alloc(8, TYPE_BYTES);
        assert!(!q.is_null());
    }

    #[test]
    fn rooted_survives_collect() {
        let mut slot: *mut u8 = lumia_alloc(32, TYPE_STRING);
        lumia_root_push(&mut slot as *mut *mut u8);
        lumia_gc_collect();
        assert!(!slot.is_null());
        // header still valid
        let h = header_from_payload(slot);
        unsafe {
            assert_eq!((*h).type_id, TYPE_STRING);
            assert_eq!((*h).size, 32);
        }
        lumia_root_pop();
    }

    #[test]
    fn write_barrier_empty_under_stw() {
        let p = lumia_alloc(8, TYPE_BYTES);
        lumia_write_barrier(p, 0, ptr::null_mut());
    }

    #[test]
    fn println_int_smoke() {
        lumia_println_int(7);
    }

    #[test]
    fn rooted_survives_soft_threshold() {
        // Lower limit temporarily via many small allocs with a rooted object.
        let mut slot: *mut u8 = lumia_alloc(64, TYPE_STRING);
        lumia_root_push(&mut slot as *mut *mut u8);
        for _ in 0..5000 {
            let _ = lumia_alloc(64, TYPE_BYTES);
        }
        assert!(!slot.is_null());
        let h = header_from_payload(slot);
        unsafe {
            assert_eq!((*h).type_id, TYPE_STRING);
            assert_eq!((*h).size, 64);
        }
        lumia_root_pop();
    }

    #[test]
    fn map_promotes_to_hash_and_looks_up() {
        let mut m: *mut u8 = ptr::null_mut();
        for i in 0..20 {
            m = lumia_map_set(m, i, i * 10);
        }
        assert!(!m.is_null());
        assert!(map_is_hash(m) || map_is_overlay(m));
        assert_eq!(map_count(m), 20);
        for i in 0..20 {
            assert_eq!(lumia_map_contains(m, i), 1);
            let opt = lumia_map_get(m, i, 0, 1);
            // Some(v) tag 0 with field
            unsafe {
                let base = opt as *const i64;
                assert_eq!(*base, 0);
                assert_eq!(*base.add(1), i * 10);
            }
        }
        assert_eq!(lumia_map_contains(m, 99), 0);
        m = lumia_map_remove(m, 5);
        assert_eq!(lumia_map_contains(m, 5), 0);
        assert_eq!(map_count(m), 19);
        // Still insertion-ordered keys without 5
        let keys = lumia_map_keys(m);
        unsafe {
            assert_eq!(*(keys as *const i64), 19);
            assert_eq!(*((keys as *const i64).add(1)), 0);
        }
    }

    #[test]
    fn map_overlay_set_avoids_full_clone() {
        let mut m: *mut u8 = ptr::null_mut();
        for i in 0..9 {
            m = lumia_map_set(m, i, i);
        }
        assert!(
            map_is_hash(m),
            "expected hash after promoting past small max"
        );
        m = lumia_map_set(m, 100, 42);
        assert!(map_is_overlay(m));
        assert_eq!(map_count(m), 10);
        assert_eq!(lumia_map_contains(m, 100), 1);
        assert_eq!(lumia_map_contains(m, 3), 1);
        // Another set extends delta (still overlay).
        m = lumia_map_set(m, 101, 7);
        assert!(map_is_overlay(m));
        unsafe {
            assert_eq!(map_overlay_dn(m), 2);
        }
        assert_eq!(map_count(m), 11);
        assert_eq!(lumia_map_contains(m, 101), 1);
    }

    #[test]
    fn set_promotes_to_hash_and_contains() {
        let mut s: *mut u8 = ptr::null_mut();
        for i in 0..20 {
            s = lumia_set_insert(s, i);
        }
        assert!(!s.is_null());
        assert!(set_is_hash(s));
        assert_eq!(unsafe { *(s as *const i64) }, 20);
        for i in 0..20 {
            assert_eq!(lumia_set_contains(s, i), 1);
            assert_eq!(unsafe { set_elem_at(s, i as usize) }, i);
        }
        assert_eq!(lumia_set_contains(s, 99), 0);
        s = lumia_set_remove(s, 5);
        assert_eq!(lumia_set_contains(s, 5), 0);
        assert_eq!(unsafe { *(s as *const i64) }, 19);
        assert_eq!(unsafe { set_elem_at(s, 0) }, 0);
        assert_eq!(unsafe { set_elem_at(s, 5) }, 6);
        // Shrink far enough to demote to linear
        for i in 0..12 {
            s = lumia_set_remove(s, i);
        }
        assert!(!set_is_hash(s));
        assert_eq!(unsafe { *(s as *const i64) }, 8);
    }

    #[test]
    fn memo_l2_hit_miss() {
        lumia_memo_l2_reset();
        let mut out = 0i64;
        assert_eq!(lumia_memo_l2_lookup(0, 1, 42, 0, 0, 0, &mut out), 0);
        lumia_memo_l2_store(0, 1, 42, 0, 0, 0, 99);
        assert_eq!(lumia_memo_l2_lookup(0, 1, 42, 0, 0, 0, &mut out), 1);
        assert_eq!(out, 99);
        assert_eq!(lumia_memo_l2_lookup(0, 1, 7, 0, 0, 0, &mut out), 0);
        // 4-arg key
        lumia_memo_l2_store(1, 4, 1, 2, 3, 4, 77);
        assert_eq!(lumia_memo_l2_lookup(1, 4, 1, 2, 3, 4, &mut out), 1);
        assert_eq!(out, 77);
        assert_eq!(lumia_memo_l2_lookup(1, 4, 1, 2, 3, 5, &mut out), 0);
        assert!(lumia_memo_l2_hits() >= 2);
        assert!(lumia_memo_l2_misses() >= 2);
        lumia_memo_l2_reset();
    }

    #[test]
    fn memo_idx_hit_miss() {
        lumia_memo_idx_reset();
        let mut out = 0i64;
        assert_eq!(lumia_memo_idx_lookup(0, 10, &mut out), 0);
        lumia_memo_idx_store(0, 10, 55);
        assert_eq!(lumia_memo_idx_lookup(0, 10, &mut out), 1);
        assert_eq!(out, 55);
        assert_eq!(lumia_memo_idx_lookup(0, 11, &mut out), 0);
        assert_eq!(lumia_memo_idx_lookup(0, -1, &mut out), 0);
        assert_eq!(lumia_memo_idx_lookup(0, MEMO_IDX_CAP as i64, &mut out), 0);
        assert!(lumia_memo_idx_hits() >= 1);
        assert!(lumia_memo_idx_misses() >= 1);
        lumia_memo_idx_reset();
    }

    #[test]
    fn range_is_iota_not_materialized() {
        let r = lumia_range(0, 1_000_000);
        assert!(!r.is_null());
        unsafe {
            assert_eq!((*header_from_payload(r)).type_id, TYPE_LIST_IOTA);
            assert_eq!((*header_from_payload(r)).size, 16);
        }
        assert_eq!(lumia_list_len(r), 1_000_000);
        assert_eq!(lumia_list_get(r, 0), 0);
        assert_eq!(lumia_list_get(r, 999_999), 999_999);
        // Content-equal to a small heap list of the same prefix.
        let h = lumia_range(10, 13);
        let forced = force_heap_list(h);
        unsafe {
            assert_eq!((*header_from_payload(forced)).type_id, TYPE_LIST);
        }
        assert_eq!(lumia_eq(h as i64, forced as i64), 1);
        assert_eq!(lumia_list_len(lumia_list_take(r, 3)), 3);
        assert_eq!(lumia_list_get(lumia_list_slice(r, 5), 0), 5);
    }

    #[test]
    fn empty_list_singleton_survives_gc() {
        let a = lumia_list_empty();
        let b = lumia_list_empty();
        assert_eq!(a, b);
        assert_eq!(lumia_list_len(a), 0);
        // Force a collection; permanent root must keep the singleton alive.
        lumia_gc_collect();
        assert_eq!(lumia_list_empty(), a);
        // Identity concat on a heap list (Iota would be forced first).
        let xs = force_heap_list(lumia_range(1, 4));
        let id = lumia_list_concat(lumia_list_empty(), xs);
        assert_eq!(id, xs);
        assert_eq!(lumia_list_len(id), 3);
        assert_eq!(lumia_list_concat(xs, lumia_list_empty()), xs);
    }

    #[test]
    #[should_panic(expected = "list too large")]
    fn force_huge_iota_traps_without_alloc() {
        // Length that cannot fit in ObjectHeader.size (u32) when stored as bytes.
        let n = (u32::MAX as i64 / 8) + 8;
        let r = lumia_range(0, n);
        let _ = force_heap_list(r);
    }

    #[test]
    #[should_panic(expected = "list too large")]
    fn list_payload_bytes_rejects_overflow() {
        let _ = list_payload_bytes(i64::MAX);
    }

    #[test]
    #[should_panic(expected = "parallel map worker")]
    fn par_worker_alloc_is_forbidden() {
        PAR_WORKER.with(|c| c.set(true));
        // Call Rust path (not `extern "C"`) so the panic can unwind for should_panic.
        let _ = MarkSweep.alloc(8, TYPE_LIST);
    }

    #[test]
    fn list_f64_eq_follows_ieee() {
        let pos0 = 0.0f64.to_bits() as i64;
        let neg0 = (-0.0f64).to_bits() as i64;
        let nan = f64::NAN.to_bits() as i64;
        let a = {
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
            unsafe {
                *(p as *mut i64) = 1;
                *((p as *mut i64).add(1)) = pos0;
            }
            p
        };
        let b = {
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
            unsafe {
                *(p as *mut i64) = 1;
                *((p as *mut i64).add(1)) = neg0;
            }
            p
        };
        let c = {
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
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
            let p = lumia_alloc(list_payload_bytes(1), TYPE_LIST_F64);
            unsafe {
                *(p as *mut i64) = 1;
                *((p as *mut i64).add(1)) = nan;
            }
            p
        };
        assert_eq!(lumia_eq(c as i64, c2 as i64), 0);
    }

    #[test]
    fn show_list_formats_elems() {
        let p = lumia_alloc(list_payload_bytes(2), TYPE_LIST);
        unsafe {
            *(p as *mut i64) = 2;
            *((p as *mut i64).add(1)) = 1;
            *((p as *mut i64).add(2)) = 2;
        }
        let s = lumia_show(p as i64);
        let text = with_str_bytes(s, |b| String::from_utf8_lossy(b).into_owned());
        assert_eq!(text, "[1, 2]");
    }

    #[test]
    fn ensure_map_vf64_accepts_empty_assoc() {
        let m = lumia_alloc(8, TYPE_MAP_ASSOC);
        unsafe {
            *(m as *mut i64) = 0;
        }
        let m2 = lumia_ensure_map_vf64(m);
        assert!(!m2.is_null());
        unsafe {
            assert_eq!((*header_from_payload(m2)).type_id, TYPE_MAP_ASSOC_VF64);
        }
        // Still assoc (no hash promotion).
        assert!(map_is_assoc(m2));
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
}

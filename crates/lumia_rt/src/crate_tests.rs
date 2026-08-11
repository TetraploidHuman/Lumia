//! Runtime integration tests (GC, map/set, memo, float eq).

use super::*;
use crate::common::{
    gc_heap_lens_for_test, gc_live_bytes_for_test, header_from_payload, set_gc_limits_for_test,
    trap_abort, PAR_WORKER,
};
use crate::gc::list_payload_bytes;
use crate::list::force_heap_list;
use crate::map_set::{
    map_count, map_is_assoc, map_is_hash, map_is_overlay, map_overlay_dn, set_elem_at, set_is_hash,
};
use crate::string_io::with_str_bytes;
use crate::MmBackend;
use std::ptr;

struct GcLimitGuard {
    young: usize,
    old: usize,
}
impl GcLimitGuard {
    fn set(young: usize, old: usize) -> Self {
        let (y, o) = (
            *crate::common::YOUNG_LIMIT
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            *crate::common::HEAP_LIMIT
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        set_gc_limits_for_test(young, old);
        Self { young: y, old: o }
    }
}
impl Drop for GcLimitGuard {
    fn drop(&mut self) {
        set_gc_limits_for_test(self.young, self.old);
    }
}

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
fn write_barrier_records_old_to_young() {
    let p = lumia_alloc(8, TYPE_BYTES);
    // Young→young is a no-op for the remembered set.
    lumia_write_barrier(p, 0, ptr::null_mut());
    assert_eq!(crate::common::gc_remembered_len_for_test(), 0);
}

#[test]
fn println_int_smoke() {
    lumia_println_int(7);
}

#[test]
fn list_append_cow_unique_grows_and_alias_copies() {
    use crate::list::{
        lumia_list_append, lumia_list_empty, lumia_list_get, lumia_list_len, lumia_list_retain,
    };
    let mut xs = lumia_list_empty();
    let mut ys: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut xs as *mut *mut u8);
    lumia_root_push(&mut ys as *mut *mut u8);
    xs = lumia_list_append(xs, 1);
    for i in 2..=64 {
        xs = lumia_list_append(xs, i);
    }
    assert_eq!(lumia_list_len(xs), 64);
    assert_eq!(lumia_list_get(xs, 0), 1);
    assert_eq!(lumia_list_get(xs, 63), 64);
    // Alias then append — old snapshot must stay length 64.
    lumia_list_retain(xs);
    ys = xs;
    xs = lumia_list_append(xs, 65);
    assert_eq!(lumia_list_len(ys), 64);
    assert_eq!(lumia_list_len(xs), 65);
    assert_eq!(lumia_list_get(xs, 64), 65);
    lumia_root_pop();
    lumia_root_pop();
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
fn minor_promotes_rooted_and_frees_garbage() {
    let _limits = GcLimitGuard::set(256, 16 * 1024 * 1024);
    let mut slot: *mut u8 = lumia_alloc(64, TYPE_STRING);
    lumia_root_push(&mut slot as *mut *mut u8);
    // Fill nursery with garbage past young limit → minor STW.
    for _ in 0..32 {
        let _ = lumia_alloc(64, TYPE_BYTES);
    }
    let (young_n, old_n) = gc_heap_lens_for_test();
    assert!(
        old_n >= 1,
        "rooted survivor should promote to old, young={young_n} old={old_n}"
    );
    assert!(!slot.is_null());
    unsafe {
        assert_eq!((*header_from_payload(slot)).type_id, TYPE_STRING);
    }
    lumia_root_pop();
}

#[test]
fn minor_keeps_young_reachable_from_old() {
    use crate::list::{lumia_list_append, lumia_list_empty, lumia_list_get, lumia_list_len};
    let _limits = GcLimitGuard::set(512, 16 * 1024 * 1024);
    // Build a unique list, promote it, then store a fresh young heap pointer in it.
    let mut xs = lumia_list_empty();
    let mut child = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child as i64);
    lumia_root_push(&mut xs as *mut *mut u8);
    for _ in 0..64 {
        let _ = lumia_alloc(64, TYPE_BYTES);
    }
    let (_y0, old0) = gc_heap_lens_for_test();
    assert!(old0 >= 1, "list should have tenured");
    // New young child stored into (possibly tenured) list via COW/in-place append.
    child = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child as i64);
    for _ in 0..64 {
        let _ = lumia_alloc(64, TYPE_BYTES);
    }
    assert_eq!(lumia_list_len(xs), 2);
    let kept = lumia_list_get(xs, 1) as *mut u8;
    assert!(
        crate::common::is_heap_payload(kept),
        "young-from-old child must survive minor"
    );
    lumia_root_pop();
    let (_y, _o) = gc_live_bytes_for_test();
}

#[test]
fn write_barrier_remembers_old_to_young() {
    use crate::list::{lumia_list_append, lumia_list_empty, lumia_list_len};
    let _limits = GcLimitGuard::set(256, 16 * 1024 * 1024);
    // Drop leftovers from earlier tests so nursery pressure is predictable.
    lumia_gc_collect();
    let mut xs = lumia_list_empty();
    let child0 = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child0 as i64);
    lumia_root_push(&mut xs as *mut *mut u8);
    let old_before = gc_heap_lens_for_test().1;
    for _ in 0..128 {
        let _ = lumia_alloc(64, TYPE_BYTES);
        if gc_heap_lens_for_test().1 > old_before {
            break;
        }
    }
    assert!(
        gc_heap_lens_for_test().1 > old_before,
        "rooted list should tenure under nursery pressure"
    );
    let before = crate::common::gc_remembered_len_for_test();
    let child1 = lumia_alloc(16, TYPE_BYTES);
    xs = lumia_list_append(xs, child1 as i64);
    let after = crate::common::gc_remembered_len_for_test();
    assert!(
        after > before,
        "in-place append into old list must dirty remembered set ({before} -> {after})"
    );
    assert_eq!(lumia_list_len(xs), 2);
    lumia_root_pop();
}

#[test]
fn map_promotes_to_hash_and_looks_up() {
    let mut m: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut m as *mut *mut u8);
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
    lumia_root_pop();
}

#[test]
fn map_overlay_set_avoids_full_clone() {
    let mut m: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut m as *mut *mut u8);
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
    lumia_root_pop();
}

#[test]
fn set_promotes_to_hash_and_contains() {
    let mut s: *mut u8 = ptr::null_mut();
    lumia_root_push(&mut s as *mut *mut u8);
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
    lumia_root_pop();
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

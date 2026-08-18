// Extracted from production module (Todo: RT 测例半迁).
use super::*;
use crate::common::{is_heap_payload, TYPE_BYTES};
use crate::gc::{lumia_alloc, lumia_gc_collect, lumia_root_pop, lumia_root_push};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn gc_sees_other_thread_roots() {
    let barrier = Arc::new(Barrier::new(2));
    let barrier_main = Arc::clone(&barrier);
    let kept = Arc::new(AtomicUsize::new(0));
    let kept_t = Arc::clone(&kept);

    let child = thread::spawn(move || {
        ensure_mutator_registered();
        let mut slot = lumia_alloc(16, TYPE_BYTES);
        assert!(!slot.is_null());
        unsafe { lumia_root_push(&mut slot as *mut *mut u8) };
        kept_t.store(slot as usize, Ordering::SeqCst);
        barrier.wait();
        // Parent runs GC while we stay rooted.
        barrier.wait();
        assert!(is_heap_payload(slot));
        lumia_root_pop();
    });

    barrier_main.wait();
    lumia_gc_collect();
    let p = kept.load(Ordering::SeqCst) as *mut u8;
    assert!(is_heap_payload(p), "child root must survive parent GC");
    barrier_main.wait();
    child.join().unwrap();
}

#[test]
fn tls_lab_bump_pending_then_flush_survives_gc() {
    ensure_mutator_registered();
    lumia_gc_collect();
    let mut slot = lumia_alloc(16, TYPE_BYTES);
    assert!(!slot.is_null());
    assert!(
        crate::mutator::tls_lab_active_for_test()
            || crate::heap::with_heap(|h| {
                let hdr = crate::common::header_from_payload(slot);
                h.nursery.is_live(hdr)
            }),
        "small alloc should use TLS LAB or process nursery"
    );
    assert!(is_heap_payload(slot));
    unsafe { lumia_root_push(&mut slot as *mut *mut u8) };
    lumia_gc_collect();
    assert!(
        is_heap_payload(slot),
        "flushed LAB object must survive collect"
    );
    lumia_root_pop();
    lumia_gc_collect();
    assert!(!is_heap_payload(slot));
}

#[test]
fn push_pop_without_heap_lock_roundtrip() {
    ensure_mutator_registered();
    let mut a: *mut u8 = std::ptr::null_mut();
    let mut b: *mut u8 = std::ptr::null_mut();
    push_root(&mut a as *mut *mut u8);
    push_root(&mut b as *mut *mut u8);
    pop_root();
    pop_root();
    let taken = take_local_roots();
    assert!(taken.is_empty());
    set_local_roots(vec![&mut a as *mut *mut u8]);
    assert_eq!(take_local_roots().len(), 1);
}

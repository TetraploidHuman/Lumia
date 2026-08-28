use super::*;
use crate::gc::{configure_mark_parallelism, gc_set_mark_quantum_for_test};

#[test]
fn mm_mode_env_and_set() {
    lumi_set_mm_mode(0);
    assert_eq!(lumi_mm_mode(), 0);
    lumi_set_mm_mode(1);
    assert_eq!(lumi_mm_mode(), 1);
    lumi_set_mm_mode(0);
}

#[test]
fn gc_stats_increment_on_collect() {
    use crate::gc::{gc_reset_stats_for_test, lumi_gc_full_count, lumi_gc_print_stats};
    gc_reset_stats_for_test();
    let p = lumi_alloc(32, TYPE_BYTES);
    assert!(!p.is_null());
    lumi_gc_collect();
    assert!(lumi_gc_full_count() >= 1);
    lumi_gc_print_stats(1);
    gc_reset_stats_for_test();
}

#[test]
fn arc_mode_free_on_zero_list() {
    use crate::list::{
        lumi_list_append, lumi_list_empty, lumi_list_len, lumi_list_release, lumi_list_retain,
    };
    lumi_set_mm_mode(1);
    assert_eq!(lumi_mm_mode(), 1);
    let mut xs = lumi_list_empty();
    lumi_root_push(&mut xs as *mut *mut u8);
    xs = lumi_list_append(xs, 1);
    xs = lumi_list_append(xs, 2);
    assert_eq!(lumi_list_len(xs), 2);
    lumi_list_retain(xs);
    lumi_list_release(xs);
    // Second release drops rc to 0 → Arc free; pointer must leave HEAP_SET.
    let doomed = xs;
    lumi_list_release(doomed);
    xs = ptr::null_mut();
    lumi_root_pop();
    assert!(
        !crate::common::is_heap_payload(doomed),
        "Arc free-on-zero should unregister list"
    );
    lumi_set_mm_mode(0);
}

#[test]
fn arc_mode_free_on_zero_bytes() {
    lumi_set_mm_mode(1);
    let p = lumi_alloc(32, TYPE_BYTES);
    assert!(!p.is_null());
    assert!(crate::common::is_heap_payload(p));
    // Alloc starts at rc=1 under Arc; retain+release leaves rc=1, then final release frees.
    lumi_heap_retain(p);
    lumi_heap_release(p);
    let doomed = p;
    lumi_heap_release(doomed);
    assert!(
        !crate::common::is_heap_payload(doomed),
        "Arc free-on-zero should unregister non-COW Bytes"
    );
    lumi_set_mm_mode(0);
}

#[test]
fn arc_mode_free_on_zero_string() {
    lumi_set_mm_mode(1);
    let p = lumi_alloc(16, TYPE_STRING);
    assert!(!p.is_null());
    lumi_heap_release(p);
    assert!(
        !crate::common::is_heap_payload(p),
        "Arc free-on-zero should unregister String"
    );
    lumi_set_mm_mode(0);
}

#[test]
fn parallel_mark_drain_collects() {
    use crate::gc::{gc_reset_stats_for_test, lumi_gc_full_count};
    gc_reset_stats_for_test();
    configure_mark_parallelism(4);
    // Allocate a small graph then force full collect under parallel drain.
    let mut roots = Vec::new();
    for _ in 0..64 {
        let p = lumi_alloc(64, TYPE_BYTES);
        assert!(!p.is_null());
        roots.push(p);
    }
    for p in &roots {
        lumi_root_push(p as *const *mut u8 as *mut *mut u8);
    }
    lumi_gc_collect();
    assert!(lumi_gc_full_count() >= 1);
    for _ in &roots {
        lumi_root_pop();
    }
    configure_mark_parallelism(1);
    gc_reset_stats_for_test();
}

#[test]
fn gc_mark_threads_scales_quantum() {
    gc_set_mark_quantum_for_test(256);
    configure_mark_parallelism(4);
    let p = lumi_alloc(16, TYPE_BYTES);
    assert!(!p.is_null());
    lumi_gc_collect();
    gc_set_mark_quantum_for_test(256);
}

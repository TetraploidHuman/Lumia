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
fn parallel_mark_drain_collects() {
    use crate::gc::{configure_mark_parallelism, gc_reset_stats_for_test, lumi_gc_full_count};
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

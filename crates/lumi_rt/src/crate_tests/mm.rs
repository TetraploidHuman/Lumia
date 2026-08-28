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
fn gc_mark_threads_scales_quantum() {
    gc_set_mark_quantum_for_test(256);
    configure_mark_parallelism(4);
    // Smoke: allocation still works after quantum scaling.
    let p = lumi_alloc(16, TYPE_BYTES);
    assert!(!p.is_null());
    lumi_gc_collect();
    gc_set_mark_quantum_for_test(256);
}

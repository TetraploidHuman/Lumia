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
fn arc_mode_full_collect_completes_stw() {
    use crate::gc::{
        gc_full_marking_for_test, gc_reset_stats_for_test, gc_set_incremental_full_for_test,
        lumi_gc_full_count,
    };
    // Prefer incremental in MS; Arc must still finish collect via STW.
    gc_set_incremental_full_for_test(true);
    lumi_set_mm_mode(1);
    gc_reset_stats_for_test();
    let p = lumi_alloc(16, TYPE_BYTES);
    assert!(!p.is_null());
    lumi_gc_collect();
    assert!(lumi_gc_full_count() >= 1);
    assert!(!gc_full_marking_for_test());
    lumi_set_mm_mode(0);
    gc_set_incremental_full_for_test(true);
}

#[test]
fn arc_mode_cycle_reclaimed_by_stw_collect() {
    use crate::common::{header_from_payload, TYPE_ADT};
    lumi_set_mm_mode(1);
    // Two ADTs forming a cycle: each holds the other as field word 1.
    // payload layout: [tag:i64][field0:i64] — 16 bytes.
    let a = lumi_alloc(16, TYPE_ADT);
    let b = lumi_alloc(16, TYPE_ADT);
    unsafe {
        *(a as *mut i64) = 0;
        *(a as *mut i64).add(1) = b as i64;
        *(b as *mut i64) = 0;
        *(b as *mut i64).add(1) = a as i64;
        // Extra retains so free-on-zero won't fire when we drop roots (cycle rc≥1).
        (*header_from_payload(a)).rc = 2;
        (*header_from_payload(b)).rc = 2;
    }
    // No roots → STW mark-sweep must reclaim the cycle despite rc>0.
    lumi_gc_collect();
    assert!(
        !crate::common::is_heap_payload(a) && !crate::common::is_heap_payload(b),
        "Arc STW collect should reclaim cyclic ADTs"
    );
    lumi_set_mm_mode(0);
}

#[test]
fn arc_cycle_candidate_threshold_triggers_collect() {
    use crate::common::{header_from_payload, value_rc_release, TYPE_ADT};
    use crate::cycle_cand::{
        cycle_cand_len_for_test, cycle_cand_set_threshold_for_test, cycle_collect_pending_for_test,
    };
    use crate::gc::{gc_reset_stats_for_test, lumi_gc_full_count};
    lumi_set_mm_mode(1);
    cycle_cand_set_threshold_for_test(1);
    gc_reset_stats_for_test();
    let a = lumi_alloc(16, TYPE_ADT);
    let b = lumi_alloc(16, TYPE_ADT);
    unsafe {
        *(a as *mut i64) = 0;
        *(a as *mut i64).add(1) = b as i64;
        *(b as *mut i64) = 0;
        *(b as *mut i64).add(1) = a as i64;
        // Cycle edges + one external alias on `a` (rc=2).
        (*header_from_payload(a)).rc = 2;
        (*header_from_payload(b)).rc = 1;
    }
    // Drop the external alias: rc 2→1 notes candidate; thresh=1 → STW collect
    // via collect_if_cycle_pending (try_borrow BACKEND).
    value_rc_release(a);
    assert!(
        !crate::common::is_heap_payload(a) && !crate::common::is_heap_payload(b),
        "cycle-candidate flush should reclaim unreachable cycle without explicit collect"
    );
    assert!(lumi_gc_full_count() >= 1);
    assert_eq!(cycle_cand_len_for_test(), 0);
    assert!(!cycle_collect_pending_for_test());
    cycle_cand_set_threshold_for_test(64);
    lumi_set_mm_mode(0);
}

#[test]
fn arc_cycle_candidate_skips_bytes() {
    use crate::cycle_cand::{cycle_cand_len_for_test, cycle_cand_set_threshold_for_test};
    lumi_set_mm_mode(1);
    cycle_cand_set_threshold_for_test(64);
    let p = lumi_alloc(16, TYPE_BYTES);
    lumi_heap_retain(p); // rc 1→2
    lumi_heap_release(p); // rc 2→1 — Bytes must not enqueue
    assert_eq!(cycle_cand_len_for_test(), 0);
    lumi_heap_release(p); // free
    lumi_set_mm_mode(0);
}

#[test]
fn heap_shared_mirror_membership() {
    use crate::heap_shared::{
        heap_shared_clear_for_test, heap_shared_contains, heap_shared_set_for_test,
    };
    heap_shared_set_for_test(true);
    let p = lumi_alloc(16, TYPE_BYTES);
    let h = crate::common::header_from_payload(p);
    assert!(heap_shared_contains(h));
    assert!(crate::common::is_heap_payload(p));
    lumi_gc_collect();
    // Unrooted → swept; mirror must drop membership.
    assert!(!heap_shared_contains(h));
    assert!(!crate::common::is_heap_payload(p));
    heap_shared_clear_for_test();
}

#[test]
fn arc_map_overlay_retains_parent() {
    use crate::common::{cow_rc_is_unique, header_from_payload, TYPE_MAP};
    use crate::map_set::{map_alloc_overlay, map_is_overlay, map_overlay_parent};
    lumi_set_mm_mode(1);
    let parent = lumi_alloc(8, TYPE_MAP);
    unsafe {
        *(parent as *mut i64) = 0; // empty linear
        // Alloc starts rc=1 under Arc; retain so overlay + external alias both live.
        crate::common::cow_rc_retain(parent, false);
        assert!(!cow_rc_is_unique(parent, false)); // rc≥2
        let overlay = map_alloc_overlay(parent, &[(1, 2)]);
        assert!(map_is_overlay(overlay));
        assert_eq!(map_overlay_parent(overlay), parent);
        // Overlay retain bumped parent again (rc≥3).
        assert!((*header_from_payload(parent)).rc >= 3);
        // Drop overlay via Arc free-on-zero (rc starts at 1).
        crate::common::cow_rc_release(overlay, false);
        assert!(
            crate::common::is_heap_payload(parent),
            "parent must survive overlay free"
        );
        // Drop the extra external retain + final alloc retain.
        crate::common::cow_rc_release(parent, false);
        crate::common::cow_rc_release(parent, false);
        assert!(!crate::common::is_heap_payload(parent));
    }
    lumi_set_mm_mode(0);
}

#[test]
fn arc_sweep_dead_slice_releases_parent_without_panic() {
    use crate::common::TYPE_LIST;
    use crate::list::lumi_list_slice;
    lumi_set_mm_mode(1);
    let parent = lumi_alloc(list_payload_bytes(2), TYPE_LIST);
    unsafe {
        *(parent as *mut i64) = 2;
        *(parent as *mut i64).add(1) = 10;
        *(parent as *mut i64).add(2) = 20;
        crate::common::list_rc_retain(parent); // rc=2
    }
    let slice = lumi_list_slice(parent, 0);
    // Drop external parent alias; parent kept alive by slice retain.
    crate::common::list_rc_release(parent);
    assert!(crate::common::is_heap_payload(parent));
    // Neither rooted — STW collect must reclaim both without RefCell panic.
    let _ = (slice, parent);
    lumi_gc_collect();
    assert!(!crate::common::is_heap_payload(slice));
    assert!(!crate::common::is_heap_payload(parent));
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

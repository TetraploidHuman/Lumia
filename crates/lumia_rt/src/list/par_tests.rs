use super::*;
use crate::list::{list_bool_elems, lumia_list_append, lumia_list_empty};
use crate::task::{lumia_scope_enter, lumia_scope_leave};
use lumia_abi::{TYPE_LIST, TYPE_LIST_BOOL, TYPE_LIST_F64};

extern "C" fn identity(x: i64) -> i64 {
    x
}

extern "C" fn always_true(_x: i64) -> i64 {
    1
}

#[test]
fn par_map_preserves_bool_result_tid() {
    let mut xs = lumia_list_empty();
    for i in 0..32 {
        xs = unsafe { lumia_list_append(xs, i) };
    }
    let out = unsafe { lumia_list_par_map(xs, Some(always_true), TYPE_LIST_BOOL) };
    assert!(!out.is_null());
    assert!(
        list_bool_elems(out),
        "par_map must keep TID_B_KEY on Bool result lists"
    );
    let empty = unsafe { lumia_list_par_map(lumia_list_empty(), Some(always_true), TYPE_LIST_BOOL) };
    assert!(list_bool_elems(empty), "empty Bool par_map must stay tagged");
}

#[test]
fn par_map_preserves_float_result_tid() {
    let mut xs = lumia_list_empty();
    for i in 0..32 {
        xs = unsafe { lumia_list_append(xs, i) };
    }
    let out = unsafe { lumia_list_par_map(xs, Some(identity), TYPE_LIST_F64) };
    assert!(!out.is_null());
    assert!(crate::list::list_float_elems(out));
}

#[test]
fn par_map_under_scope_counts_task_demotion() {
    let _g = crate::task::scheduler::sched_test_guard();
    reset_par_task_demotions();
    let mut xs = lumia_list_empty();
    for i in 0..32 {
        xs = unsafe { lumia_list_append(xs, i) };
    }
    lumia_scope_enter(0);
    let out = unsafe { lumia_list_par_map(xs, Some(identity), TYPE_LIST) };
    lumia_scope_leave();
    assert!(!out.is_null());
    assert!(
        par_task_demotions() >= 1,
        "expected demotion under active scope, got {}",
        par_task_demotions()
    );
}

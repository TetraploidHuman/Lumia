use super::*;
use crate::list::{lumia_list_append, lumia_list_empty};
use crate::task::{lumia_scope_enter, lumia_scope_leave};
use lumia_abi::TYPE_LIST;

extern "C" fn identity(x: i64) -> i64 {
    x
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

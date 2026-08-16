//! Visible Task/Channel gate for `crate_tests` (complements `task::fiber` unit tests).

use crate::task::{
    lumia_channel_close, lumia_channel_new, lumia_channel_recv, lumia_channel_send,
    lumia_scope_enter, lumia_scope_leave, lumia_task_join, lumia_task_spawn,
    lumia_task_spawn_nullary,
};

extern "C" fn forty_two() -> i64 {
    42
}

extern "C" fn send_pair(env: i64) -> i64 {
    let ch = env as *mut u8;
    lumia_channel_send(ch, 7);
    lumia_channel_send(ch, 8);
    lumia_channel_close(ch);
    0
}

#[test]
fn crate_tests_spawn_join_nullary() {
    lumia_scope_enter(0);
    let t = lumia_task_spawn_nullary(Some(forty_two));
    let v = lumia_task_join(t);
    lumia_scope_leave();
    assert_eq!(v, 42);
}

#[test]
fn crate_tests_channel_send_recv() {
    lumia_scope_enter(0);
    let ch = lumia_channel_new(2);
    let _ = lumia_task_spawn(Some(send_pair), ch as i64);
    assert_eq!(lumia_channel_recv(ch), 7);
    assert_eq!(lumia_channel_recv(ch), 8);
    lumia_scope_leave();
}

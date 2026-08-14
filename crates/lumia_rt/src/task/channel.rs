//! Bounded channel with fiber-aware send/recv.

use super::scheduler::{
    alloc_id, assert_task_api_allowed, check_current_not_cancelled, current_fiber_cancelled_locked,
    current_waiter, park_until, push_waiter_unique, suspend_current, wake, wake_many, with_sched,
    ChannelState, Waiter,
};
use crate::common::trap_abort;
use crate::gc::{lumia_alloc, lumia_root_pop, lumia_root_push};
use crate::heap::with_heap;
use lumia_abi::TYPE_CHANNEL;
use std::collections::VecDeque;

fn channel_id(handle: *mut u8) -> u64 {
    if handle.is_null() {
        trap_abort("lumia: null channel");
    }
    unsafe { *(handle as *const i64) as u64 }
}

/// Create a bounded channel (`capacity >= 1`). Returns TYPE_CHANNEL handle.
#[no_mangle]
pub extern "C" fn lumia_channel_new(capacity: i64) -> *mut u8 {
    assert_task_api_allowed();
    if capacity < 1 {
        trap_abort("lumia: channel capacity must be >= 1");
    }
    let id = alloc_id();
    let p = lumia_alloc(8, TYPE_CHANNEL);
    unsafe {
        *(p as *mut i64) = id as i64;
    }
    let mut root_slot = p;
    lumia_root_push(&mut root_slot as *mut *mut u8);
    with_sched(|s| {
        s.channels.insert(
            id,
            ChannelState {
                cap: capacity as usize,
                buf: VecDeque::with_capacity(capacity as usize),
                closed: false,
                send_waiters: VecDeque::new(),
                recv_waiters: VecDeque::new(),
            },
        );
    });
    lumia_root_pop();
    crate::task::scheduler::lumia_abi_handoff_set(p as i64);
    p
}

fn try_reap_channel(id: u64) {
    with_sched(|s| {
        let map = &mut s.channels;
        let reap = map.get(&id).is_some_and(|ch| {
            ch.closed
                && ch.buf.is_empty()
                && ch.send_waiters.is_empty()
                && ch.recv_waiters.is_empty()
        });
        if reap {
            map.remove(&id);
        }
    });
}

#[no_mangle]
pub extern "C" fn lumia_channel_close(handle: *mut u8) {
    assert_task_api_allowed();
    let id = channel_id(handle);
    let waiters = with_sched(|s| {
        let map = &mut s.channels;
        let Some(ch) = map.get_mut(&id) else {
            return VecDeque::new();
        };
        if ch.closed {
            return VecDeque::new();
        }
        ch.closed = true;
        let mut w = std::mem::take(&mut ch.recv_waiters);
        w.extend(std::mem::take(&mut ch.send_waiters));
        w
    });
    wake_many(waiters);
    try_reap_channel(id);
}

#[no_mangle]
pub extern "C" fn lumia_channel_send(handle: *mut u8, value: i64) {
    assert_task_api_allowed();
    let id = channel_id(handle);

    loop {
        enum Step {
            Done,
            Wait,
            FailClosed,
            Cancelled,
        }
        let (step, to_wake) = with_heap(|h| {
            let full = h.full_marking;
            with_sched(|s| {
                if current_fiber_cancelled_locked(s) {
                    return (Step::Cancelled, None);
                }
                let map = &mut s.channels;
                let Some(ch) = map.get_mut(&id) else {
                    return (Step::FailClosed, None);
                };
                if ch.closed {
                    return (Step::FailClosed, None);
                }
                if ch.buf.len() < ch.cap {
                    ch.buf.push_back(value);
                    if full {
                        // Dijkstra: mutator write into a GC root set.
                        crate::gc::mark_value(value);
                    }
                    let w = ch.recv_waiters.pop_front();
                    return (Step::Done, w);
                }
                push_waiter_unique(&mut ch.send_waiters, current_waiter());
                (Step::Wait, None)
            })
        });
        if let Some(w) = to_wake {
            wake(w);
        }
        match step {
            Step::Done => return,
            Step::FailClosed => trap_abort("lumia: send on closed channel"),
            Step::Cancelled => trap_abort("lumia: task cancelled"),
            Step::Wait => {
                check_current_not_cancelled();
                match current_waiter() {
                    Waiter::Fiber(_) => {
                        suspend_current();
                        check_current_not_cancelled();
                    }
                    Waiter::Main => park_until(|| {
                        with_sched(|s| {
                            s.channels
                                .get(&id)
                                .map(|ch| ch.closed || ch.buf.len() < ch.cap)
                                .unwrap_or(true)
                        })
                    }),
                }
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_channel_recv(handle: *mut u8) -> i64 {
    assert_task_api_allowed();
    let id = channel_id(handle);
    let tid = std::thread::current().id();
    loop {
        enum Step {
            /// Value published to `SchedCore.abi_handoff` (survives across `return`).
            Value(i64),
            Wait,
            ClosedEmpty,
            Cancelled,
        }
        let (step, to_wake) = with_heap(|h| {
            let full = h.full_marking;
            with_sched(|s| {
                if current_fiber_cancelled_locked(s) {
                    return (Step::Cancelled, None);
                }
                let map = &mut s.channels;
                let Some(ch) = map.get_mut(&id) else {
                    return (Step::ClosedEmpty, None);
                };
                if let Some(v) = ch.buf.pop_front() {
                    // Pin until the next ABI overwrites/clears this thread's handoff.
                    if full {
                        crate::gc::mark_value(v);
                    }
                    s.abi_handoff.insert(tid, v);
                    let w = ch.send_waiters.pop_front();
                    return (Step::Value(v), w);
                }
                if ch.closed {
                    return (Step::ClosedEmpty, None);
                }
                push_waiter_unique(&mut ch.recv_waiters, current_waiter());
                (Step::Wait, None)
            })
        });
        if let Some(w) = to_wake {
            wake(w);
        }
        match step {
            Step::Value(v) => return v,
            Step::ClosedEmpty => {
                try_reap_channel(id);
                trap_abort("lumia: recv on closed empty channel");
            }
            Step::Cancelled => trap_abort("lumia: task cancelled"),
            Step::Wait => {
                check_current_not_cancelled();
                match current_waiter() {
                    Waiter::Fiber(_) => {
                        suspend_current();
                        check_current_not_cancelled();
                    }
                    Waiter::Main => park_until(|| {
                        with_sched(|s| {
                            s.channels
                                .get(&id)
                                .map(|ch| ch.closed || !ch.buf.is_empty())
                                .unwrap_or(true)
                        })
                    }),
                }
            }
        }
    }
}

/// ABI: returns i64 value; if none, returns with `*out_ok = 0`, else `*out_ok = 1`.
#[no_mangle]
pub extern "C" fn lumia_channel_recv_opt(handle: *mut u8, out_ok: *mut i64) -> i64 {
    assert_task_api_allowed();
    if out_ok.is_null() {
        trap_abort("lumia: recv_opt null out_ok");
    }
    let id = channel_id(handle);
    let tid = std::thread::current().id();
    loop {
        enum Step {
            Value(i64),
            None,
            Wait,
            Cancelled,
        }
        let (step, to_wake) = with_heap(|h| {
            let full = h.full_marking;
            with_sched(|s| {
                if current_fiber_cancelled_locked(s) {
                    return (Step::Cancelled, None);
                }
                let map = &mut s.channels;
                let Some(ch) = map.get_mut(&id) else {
                    s.abi_handoff.remove(&tid);
                    return (Step::None, None);
                };
                if let Some(v) = ch.buf.pop_front() {
                    if full {
                        crate::gc::mark_value(v);
                    }
                    s.abi_handoff.insert(tid, v);
                    let w = ch.send_waiters.pop_front();
                    return (Step::Value(v), w);
                }
                if ch.closed {
                    s.abi_handoff.remove(&tid);
                    return (Step::None, None);
                }
                push_waiter_unique(&mut ch.recv_waiters, current_waiter());
                (Step::Wait, None)
            })
        });
        if let Some(w) = to_wake {
            wake(w);
        }
        match step {
            Step::Value(v) => {
                unsafe {
                    *out_ok = 1;
                }
                return v;
            }
            Step::None => {
                unsafe {
                    *out_ok = 0;
                }
                with_sched(|s| {
                    s.abi_handoff.remove(&tid);
                });
                try_reap_channel(id);
                return 0;
            }
            Step::Cancelled => trap_abort("lumia: task cancelled"),
            Step::Wait => {
                check_current_not_cancelled();
                match current_waiter() {
                    Waiter::Fiber(_) => {
                        suspend_current();
                        check_current_not_cancelled();
                    }
                    Waiter::Main => park_until(|| {
                        with_sched(|s| {
                            s.channels
                                .get(&id)
                                .map(|ch| ch.closed || !ch.buf.is_empty())
                                .unwrap_or(true)
                        })
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::snapshot_sched_gc_roots;

    #[test]
    fn send_recv_buffer() {
        let ch = lumia_channel_new(2);
        lumia_channel_send(ch, 1);
        lumia_channel_send(ch, 2);
        assert_eq!(lumia_channel_recv(ch), 1);
        assert_eq!(lumia_channel_recv(ch), 2);
        lumia_channel_close(ch);
        let mut ok = 0i64;
        let _ = lumia_channel_recv_opt(ch, &mut ok);
        assert_eq!(ok, 0);
    }

    #[test]
    fn channel_handle_not_immortal_in_sched_snapshot() {
        let ch = lumia_channel_new(1);
        let handle_bits = ch as i64;
        // Spawn/new publish abi_handoff; clear so we only assert SchedCore.channels.
        let tid = std::thread::current().id();
        with_sched(|s| {
            s.abi_handoff.remove(&tid);
        });
        let (_, vals) = snapshot_sched_gc_roots();
        assert!(
            !vals.contains(&handle_bits),
            "channel handle must not be immortal-pinned by SchedCore"
        );
        lumia_channel_send(ch, 77);
        let (_, vals) = snapshot_sched_gc_roots();
        assert!(
            vals.contains(&77),
            "buffered channel values remain GC roots"
        );
        assert_eq!(lumia_channel_recv(ch), 77);
        lumia_channel_close(ch);
        let mut ok = 0i64;
        let _ = lumia_channel_recv_opt(ch, &mut ok);
        assert_eq!(ok, 0);
    }
}

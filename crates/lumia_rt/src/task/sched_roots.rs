//! Scheduler-owned GC root park / snapshot (heap → sched lock order).

use crate::common::CALL_STACK;
use crate::heap::with_heap;
use crate::mutator::{set_local_roots, take_local_roots};

use super::sched_core::{with_sched, FiberId, PendingSpawn, SchedCore};
use super::scheduler::{
    recycle_scope_stack, refresh_scope_kind_cache, SCOPE_KIND_CACHE, SCOPE_STACK,
};

pub(super) fn discard_parked_scope_stack(s: &mut SchedCore, fid: FiberId) {
    if let Some(v) = s.parked_scope_stacks.remove(&fid) {
        recycle_scope_stack(v);
    }
}

pub(super) fn save_fiber_roots(fid: FiberId) {
    // Hold heap across TLS take + sched publish so GC cannot miss roots in flight.
    with_heap(|_| {
        let roots = take_local_roots();
        let frames = CALL_STACK.with(|s| std::mem::take(&mut *s.borrow_mut()));
        let scopes = SCOPE_STACK.with(|s| std::mem::take(&mut *s.borrow_mut()));
        SCOPE_KIND_CACHE.with(|c| c.set(0));
        with_sched(|s| {
            if roots.is_empty() {
                s.parked_roots.remove(&fid);
            } else {
                s.parked_roots.insert(fid, roots.into());
            }
            if frames.is_empty() {
                s.parked_call_stacks.remove(&fid);
            } else {
                s.parked_call_stacks.insert(fid, frames.into());
            }
            if scopes.is_empty() {
                discard_parked_scope_stack(s, fid);
            } else if let Some(old) = s.parked_scope_stacks.insert(fid, scopes) {
                recycle_scope_stack(old);
            }
        });
    });
}

pub(super) fn load_fiber_roots(fid: FiberId) {
    with_heap(|_| {
        let (roots, frames, scopes) = with_sched(|s| {
            (
                s.parked_roots
                    .remove(&fid)
                    .unwrap_or_default()
                    .into_vec(),
                s.parked_call_stacks
                    .remove(&fid)
                    .unwrap_or_default()
                    .into_vec(),
                s.parked_scope_stacks.remove(&fid).unwrap_or_default(),
            )
        });
        set_local_roots(roots);
        CALL_STACK.with(|s| *s.borrow_mut() = frames);
        SCOPE_STACK.with(|s| {
            let old = std::mem::replace(&mut *s.borrow_mut(), scopes);
            recycle_scope_stack(old);
        });
        refresh_scope_kind_cache();
    });
}

pub(super) fn restore_host_roots() {
    let tid = std::thread::current().id();
    with_heap(|_| {
        let (roots, frames, scopes) = with_sched(|s| {
            (
                s.host_roots.remove(&tid),
                s.host_call_stacks.remove(&tid),
                s.host_scope_stacks.remove(&tid),
            )
        });
        if let Some(roots) = roots {
            set_local_roots(roots.into_vec());
        }
        if let Some(frames) = frames {
            CALL_STACK.with(|s| *s.borrow_mut() = frames.into_vec());
        }
        if let Some(scopes) = scopes {
            SCOPE_STACK.with(|s| {
                let old = std::mem::replace(&mut *s.borrow_mut(), scopes);
                recycle_scope_stack(old);
            });
        } else {
            SCOPE_STACK.with(|s| {
                let old = std::mem::take(&mut *s.borrow_mut());
                recycle_scope_stack(old);
            });
        }
        refresh_scope_kind_cache();
    });
}

/// Snapshot scheduler-owned GC roots.
///
/// Prefer calling while already holding the heap lock (`with_heap`), nesting
/// `with_sched` underneath (heap → sched). Park/unpark uses the same order.
///
/// Returns `(rooted_payload_bits, task_channel_values)`. Parked/host slots are
/// dereferenced under the sched lock so later mark does not touch fiber stacks.
pub fn snapshot_sched_gc_roots() -> (Vec<i64>, Vec<i64>) {
    with_sched(|s| {
        let mut rooted = Vec::new();
        for roots in s.parked_roots.values().chain(s.host_roots.values()) {
            for &slot in roots.iter() {
                // Safety: slot addresses are live while the owning fiber/host is
                // parked in these maps; resume removes entries before mutating TLS.
                let p = unsafe { *slot };
                rooted.push(p as i64);
            }
        }
        let mut vals = Vec::new();
        for ch in s.channels.values() {
            vals.extend(ch.buf.iter().copied());
            // Channel handles live only via mutator roots (not immortal-pinned here).
        }
        for st in s.tasks.values() {
            if st.env != 0 {
                vals.push(st.env);
            }
            // Pin result only until the first successful join; afterwards the
            // TYPE_TASK handle word (scanned from mutator roots) owns it.
            if st.result_gc_pin {
                if let Some(v) = st.result {
                    vals.push(v);
                }
            }
            // Do not immortal-pin `st.handle` — live handles are mutator roots.
        }
        for slot in s.fibers.values() {
            if let Some(PendingSpawn::Unary(_, e)) = slot.pending {
                if e != 0 {
                    vals.push(e);
                }
            }
        }
        vals.extend(s.abi_handoff.values().copied());
        (rooted, vals)
    })
}

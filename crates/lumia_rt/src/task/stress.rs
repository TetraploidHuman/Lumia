//! Coroutine / Task / Channel correctness stress (BUILD §7.6).
//!
//! These are RT-level checks (no Lumia frontend). Language-level coverage lives
//! in `examples/task_*.lm` + `examples/bench_task.lm`.

#[cfg(test)]
mod tests {
    use crate::gc::{lumia_alloc, lumia_gc_collect};
    use crate::task::channel::{
        lumia_channel_close, lumia_channel_new, lumia_channel_recv, lumia_channel_send,
    };
    use crate::task::fiber::{
        lumia_scope_enter, lumia_scope_leave, lumia_task_join, lumia_task_spawn_nullary, task_join,
        task_spawn,
    };
    use crate::task::scheduler::{
        await_sched_quiescent_for_test, cancel_scope_children, lumia_scheduler_drain, with_sched,
        SCHEDULER_WORKER,
    };
    use lumia_abi::TYPE_LIST;
    use std::sync::atomic::{AtomicI64, Ordering};

    extern "C" fn add_one(env: i64) -> i64 {
        env + 1
    }

    extern "C" fn identity(env: i64) -> i64 {
        env
    }

    extern "C" fn send_n(env: i64) -> i64 {
        // env packs (ch_ptr << 0) — we pass ch as i64 and a fixed payload via TLS counter.
        lumia_channel_send(env as *mut u8, NEXT_SEND.fetch_add(1, Ordering::Relaxed));
        0
    }

    static NEXT_SEND: AtomicI64 = AtomicI64::new(1);

    struct ChPair {
        req: *mut u8,
        resp: *mut u8,
    }

    extern "C" fn echo_worker(env: i64) -> i64 {
        let pair = unsafe { &*(env as *const ChPair) };
        for _ in 0..200 {
            let x = lumia_channel_recv(pair.req);
            lumia_channel_send(pair.resp, x);
        }
        0
    }

    extern "C" fn ping_client(env: i64) -> i64 {
        let pair = unsafe { &*(env as *const ChPair) };
        let mut sum = 0i64;
        for i in 1..=200 {
            lumia_channel_send(pair.req, i);
            sum += lumia_channel_recv(pair.resp);
        }
        sum
    }

    #[test]
    fn stress_spawn_join_many() {
        const N: i64 = 500;
        lumia_scope_enter(0);
        let mut handles = Vec::with_capacity(N as usize);
        for i in 0..N {
            handles.push(task_spawn(add_one, i));
        }
        let mut sum = 0i64;
        for h in handles {
            sum += task_join(h);
        }
        lumia_scope_leave();
        // sum_{i=0}^{N-1} (i+1) = N*(N+1)/2
        assert_eq!(sum, N * (N + 1) / 2);
    }

    #[test]
    fn stress_leave_joins_fire_and_forget_many() {
        const N: i64 = 300;
        lumia_scope_enter(0);
        for i in 0..N {
            let _ = task_spawn(identity, i);
        }
        lumia_scope_leave();
        await_sched_quiescent_for_test();
    }

    #[test]
    fn stress_channel_fan_in() {
        const N: i64 = 128;
        NEXT_SEND.store(1, Ordering::Relaxed);
        lumia_scope_enter(0);
        let ch = lumia_channel_new(32);
        for _ in 0..N {
            let _ = task_spawn(send_n, ch as i64);
        }
        let mut sum = 0i64;
        for _ in 0..N {
            sum += lumia_channel_recv(ch);
        }
        lumia_channel_close(ch);
        lumia_scope_leave();
        assert_eq!(sum, N * (N + 1) / 2);
    }

    extern "C" fn block_recv(env: i64) -> i64 {
        lumia_channel_recv(env as *mut u8)
    }

    #[test]
    fn stress_channel_pingpong() {
        // Two channels: a single shared channel allows the client to recv its own
        // send before echo runs (capacity>0), leaving echo blocked forever.
        lumia_scope_enter(0);
        let req = lumia_channel_new(1);
        let resp = lumia_channel_new(1);
        let pair = Box::into_raw(Box::new(ChPair { req, resp }));
        let env = pair as i64;
        let echo = task_spawn(echo_worker, env);
        let client = task_spawn(ping_client, env);
        let _ = task_join(echo);
        let sum = task_join(client);
        unsafe {
            drop(Box::from_raw(pair));
        }
        lumia_scope_leave();
        assert_eq!(sum, 200 * 201 / 2);
    }

    #[test]
    fn stress_cancel_many_blocked() {
        const N: usize = 64;
        lumia_scope_enter(0);
        let ch = lumia_channel_new(1);
        for _ in 0..N {
            let _ = task_spawn(block_recv, ch as i64);
        }
        lumia_scheduler_drain();
        cancel_scope_children();
        lumia_scheduler_drain();
        await_sched_quiescent_for_test();
        lumia_scope_leave();
    }

    #[test]
    fn stress_nested_scopes_fan_in() {
        lumia_scope_enter(0);
        lumia_scope_enter(SCHEDULER_WORKER);
        let ch = lumia_channel_new(16);
        NEXT_SEND.store(1, Ordering::Relaxed);
        for _ in 0..40 {
            let _ = task_spawn(send_n, ch as i64);
        }
        lumia_scope_enter(0);
        for _ in 0..40 {
            let _ = task_spawn(send_n, ch as i64);
        }
        let mut sum = 0i64;
        for _ in 0..80 {
            sum += lumia_channel_recv(ch);
        }
        lumia_scope_leave();
        lumia_scope_leave();
        lumia_scope_leave();
        assert_eq!(sum, 80 * 81 / 2);
    }

    #[test]
    fn stress_gc_during_channel_traffic() {
        NEXT_SEND.store(1, Ordering::Relaxed);
        lumia_scope_enter(0);
        let ch = lumia_channel_new(8);
        for _ in 0..48 {
            let _ = task_spawn(send_n, ch as i64);
        }
        // Allocate churn + collect while tasks are live / buffered.
        for _ in 0..20 {
            let _ = lumia_alloc(64, TYPE_LIST);
            lumia_gc_collect();
        }
        let mut sum = 0i64;
        for _ in 0..48 {
            sum += lumia_channel_recv(ch);
        }
        lumia_scope_leave();
        assert_eq!(sum, 48 * 49 / 2);
    }

    #[test]
    fn stress_nullary_spawn_storm() {
        const N: usize = 400;
        extern "C" fn seven() -> i64 {
            7
        }
        lumia_scope_enter(0);
        let mut hs = Vec::with_capacity(N);
        for _ in 0..N {
            hs.push(lumia_task_spawn_nullary(Some(seven)));
        }
        let mut sum = 0i64;
        for h in hs {
            sum += lumia_task_join(h);
        }
        lumia_scope_leave();
        assert_eq!(sum, 7 * N as i64);
    }

    /// Wall-clock smoke (not a gate): print timings under `--nocapture`.
    #[test]
    fn stress_timing_smoke() {
        use std::time::Instant;
        let t0 = Instant::now();
        stress_spawn_join_many_inner(200);
        let join_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let t1 = Instant::now();
        stress_fan_in_inner(100);
        let fan_ms = t1.elapsed().as_secs_f64() * 1000.0;
        eprintln!("task_stress timing: spawn_join_200={join_ms:.2}ms fan_in_100={fan_ms:.2}ms");
        assert!(join_ms < 30_000.0 && fan_ms < 30_000.0);
    }

    fn stress_spawn_join_many_inner(n: i64) {
        lumia_scope_enter(0);
        let mut handles = Vec::with_capacity(n as usize);
        for i in 0..n {
            handles.push(task_spawn(add_one, i));
        }
        let mut sum = 0i64;
        for h in handles {
            sum += task_join(h);
        }
        lumia_scope_leave();
        assert_eq!(sum, n * (n + 1) / 2);
    }

    fn stress_fan_in_inner(n: i64) {
        NEXT_SEND.store(1, Ordering::Relaxed);
        lumia_scope_enter(0);
        let ch = lumia_channel_new(32);
        for _ in 0..n {
            let _ = task_spawn(send_n, ch as i64);
        }
        let mut sum = 0i64;
        for _ in 0..n {
            sum += lumia_channel_recv(ch);
        }
        lumia_scope_leave();
        assert_eq!(sum, n * (n + 1) / 2);
    }
}

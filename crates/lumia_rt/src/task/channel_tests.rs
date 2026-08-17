    use super::*;
    use crate::task::snapshot_sched_gc_roots;

    #[test]
    fn send_recv_buffer() {
        let ch = lumia_channel_new(2);
        unsafe { lumia_channel_send(ch, 1) };
        unsafe { lumia_channel_send(ch, 2) };
        assert_eq!(unsafe { lumia_channel_recv(ch) }, 1);
        assert_eq!(unsafe { lumia_channel_recv(ch) }, 2);
        unsafe { lumia_channel_close(ch) };
        let mut ok = 0i64;
        let _ = unsafe { lumia_channel_recv_opt(ch, &mut ok) };
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
        unsafe { lumia_channel_send(ch, 77) };
        let (_, vals) = snapshot_sched_gc_roots();
        assert!(
            vals.contains(&77),
            "buffered channel values remain GC roots"
        );
        assert_eq!(unsafe { lumia_channel_recv(ch) }, 77);
        unsafe { lumia_channel_close(ch) };
        let mut ok = 0i64;
        let _ = unsafe { lumia_channel_recv_opt(ch, &mut ok) };
        assert_eq!(ok, 0);
    }

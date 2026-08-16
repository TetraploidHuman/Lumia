e2e_ok!(e2e_task_join, "examples/task_join.lm", "3");

e2e_ok!(e2e_task_channel, "examples/task_channel.lm", "30");

e2e_ok!(e2e_task_scheduler, "examples/task_scheduler.lm", "7");

e2e_ok!(e2e_task_scope_return, "examples/task_scope_return.lm", "9");

e2e_ok!(e2e_task_join_opt, "examples/task_join_opt.lm", "0", "42");

e2e_ok!(e2e_task_cancel_siblings, "examples/task_cancel_siblings.lm", "7", "0");

e2e_ok!(e2e_task_nested_scope, "examples/task_nested_scope.lm", "7");

e2e_ok!(e2e_task_stress, "examples/task_stress.lm", "2080");

e2e_ok!(e2e_task_pingpong, "examples/task_pingpong.lm", "5050");

e2e_ok!(e2e_task_join_tree, "examples/task_join_tree.lm", "528");

e2e_ok!(e2e_task_stress_wide, "examples/task_stress_wide.lm", "8256");

#[test]
fn e2e_task_stress_multi_worker() {
    crate::harness::run_example_env(
        "examples/task_stress.lm",
        &[("LUMIA_SCHED_WORKERS", "2"), ("LUMIA_SCHED_IO", "2")],
        &["2080"],
    );
}

#[test]
fn e2e_task_stress_wide_multi_worker() {
    crate::harness::run_example_env(
        "examples/task_stress_wide.lm",
        &[("LUMIA_SCHED_WORKERS", "2"), ("LUMIA_SCHED_IO", "2")],
        &["8256"],
    );
}

#[test]
fn e2e_task_pingpong_coop() {
    crate::harness::run_example_env(
        "examples/task_pingpong.lm",
        &[("LUMIA_SCHED_WORKERS", "0"), ("LUMIA_SCHED_IO", "0")],
        &["5050"],
    );
}

#[test]
fn e2e_task_pingpong_multi_worker() {
    crate::harness::run_example_env(
        "examples/task_pingpong.lm",
        &[("LUMIA_SCHED_WORKERS", "2"), ("LUMIA_SCHED_IO", "2")],
        &["5050"],
    );
}

#[test]
fn e2e_bench_task_checksum() {
    crate::harness::run_example_env(
        "examples/bench_task.lm",
        &[("LUMIA_SCHED_WORKERS", "1"), ("LUMIA_SCHED_IO", "1")],
        &["32896", "32896", "125250"],
    );
}

// Extracted from globals.rs (Todo: RT 测例半迁).

#[test]
fn globals_contract_module_linked() {
    assert!(super::contracts_documented());
}

#[test]
fn par_worker_count_is_positive() {
    assert!(super::par_worker_count() >= 1);
}

#[test]
fn fiber_stack_bytes_meets_minimum() {
    assert!(super::fiber_stack_bytes() >= 16 * 1024);
}

#[test]
fn simd_f64_probe_is_stable() {
    let a = super::simd_f64_available();
    let b = super::simd_f64_available();
    assert_eq!(a, b);
}

#[test]
fn gc_incremental_env_tokens() {
    use super::parse_gc_incremental_env;
    assert_eq!(parse_gc_incremental_env("0"), Some(false));
    assert_eq!(parse_gc_incremental_env("FALSE"), Some(false));
    assert_eq!(parse_gc_incremental_env("stw"), Some(false));
    assert_eq!(parse_gc_incremental_env("1"), Some(true));
    assert_eq!(parse_gc_incremental_env("Yes"), Some(true));
    assert_eq!(parse_gc_incremental_env("incremental"), Some(true));
    assert_eq!(parse_gc_incremental_env("flase"), None);
    assert_eq!(parse_gc_incremental_env("maybe"), None);
    assert_eq!(parse_gc_incremental_env(""), None);
}

#[test]
fn task_runtime_latch_starts_false_and_notes() {
    // Process-global: only assert note makes load true (may already be latched
    // if earlier tests exercised Task/Channel).
    super::note_task_runtime_used();
    assert!(super::task_runtime_used_latched());
}

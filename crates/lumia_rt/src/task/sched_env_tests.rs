// Extracted from task/sched_env.rs (Todo: RT 测例半迁).
use super::parse_env_usize;

#[test]
fn parse_env_usize_accepts_integers() {
    std::env::set_var("LUMIA_TEST_SCHED_PARSE", "4");
    assert_eq!(parse_env_usize("LUMIA_TEST_SCHED_PARSE", 1), 4);
    std::env::set_var("LUMIA_TEST_SCHED_PARSE", "0");
    assert_eq!(parse_env_usize("LUMIA_TEST_SCHED_PARSE", 1), 0);
    std::env::remove_var("LUMIA_TEST_SCHED_PARSE");
}

#[test]
fn parse_env_usize_rejects_garbage_with_default() {
    std::env::set_var("LUMIA_TEST_SCHED_PARSE", "notanumber");
    assert_eq!(parse_env_usize("LUMIA_TEST_SCHED_PARSE", 7), 7);
    std::env::set_var("LUMIA_TEST_SCHED_PARSE", "  ");
    assert_eq!(parse_env_usize("LUMIA_TEST_SCHED_PARSE", 7), 7);
    std::env::remove_var("LUMIA_TEST_SCHED_PARSE");
}

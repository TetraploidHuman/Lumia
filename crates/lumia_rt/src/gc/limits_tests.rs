use crate::globals::parse_gc_incremental_env;

#[test]
fn parses_known_truthy_and_falsy() {
    assert_eq!(parse_gc_incremental_env("0"), Some(false));
    assert_eq!(parse_gc_incremental_env("FALSE"), Some(false));
    assert_eq!(parse_gc_incremental_env("stw"), Some(false));
    assert_eq!(parse_gc_incremental_env("1"), Some(true));
    assert_eq!(parse_gc_incremental_env("Yes"), Some(true));
    assert_eq!(parse_gc_incremental_env("incremental"), Some(true));
}

#[test]
fn rejects_typos_instead_of_silent_enable() {
    // Previously any non-false-ish string enabled incremental (incl. `flase`).
    assert_eq!(parse_gc_incremental_env("flase"), None);
    assert_eq!(parse_gc_incremental_env("maybe"), None);
    assert_eq!(parse_gc_incremental_env(""), None);
}

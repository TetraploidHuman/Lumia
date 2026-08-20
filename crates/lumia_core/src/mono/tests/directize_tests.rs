// Keep directize unit tests in the `mono/tests/` “test swamp” directory.
//
// We use `include!` so the test module is still nested under
// `mono::directize` (privacy: `super::...` remains valid).
include!("../directize_tests.rs");


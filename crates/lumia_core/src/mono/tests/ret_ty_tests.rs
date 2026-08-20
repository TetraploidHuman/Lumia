// Keep ret_ty unit tests in the `mono/tests/` “test swamp” directory.
//
// We use `include!` so the test module is still nested under
// `mono::ret_ty` (privacy: `super::...` remains valid).
include!("../ret_ty_tests.rs");


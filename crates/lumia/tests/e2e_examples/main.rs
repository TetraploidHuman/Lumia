//! Cross-platform e2e: build each example with `lumia` and check stdout.
//!
//! Run: `cargo test -p lumia --test e2e_examples`

#[macro_use]
mod harness;

use crate::harness::{
    e2e_exe, lumia_bin, run_check, run_example, run_example_trust_foreign_pure,
    run_example_with_stdin, workspace_root,
};
use std::process::Command;

include!("basic.rs");
include!("tco.rs");
include!("traits.rs");
include!("poly.rs");
include!("floats.rs");
include!("memo_par.rs");
include!("ffi.rs");
include!("reject.rs");
include!("regress.rs");
include!("syntax.rs");

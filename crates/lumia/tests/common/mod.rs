//! Shared helpers for `lumia` integration tests (e2e + opt_correctness).

use std::path::PathBuf;

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

pub fn lumia_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lumia"))
}

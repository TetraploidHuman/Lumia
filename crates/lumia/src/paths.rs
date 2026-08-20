//! Workspace / install path helpers (not part of the runtime ABI contract).

use std::path::{Path, PathBuf};

/// Repo root given a workspace crate's `CARGO_MANIFEST_DIR` (`crates/<name>` → `…/Lumia`).
#[inline]
pub fn workspace_root(manifest_dir: impl AsRef<Path>) -> PathBuf {
    manifest_dir.as_ref().join("../..")
}

/// Like [`workspace_root`], but `canonicalize`s when the path exists.
#[inline]
pub fn workspace_root_canonical(manifest_dir: impl AsRef<Path>) -> PathBuf {
    let p = workspace_root(manifest_dir);
    p.canonicalize().unwrap_or(p)
}

/// `std/` directory: `LUMIA_STD` if set, else `<workspace>/std`.
pub fn std_dir(manifest_dir: impl AsRef<Path>) -> PathBuf {
    if let Ok(p) = std::env::var("LUMIA_STD") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    workspace_root(manifest_dir).join("std")
}

/// `extras/` directory: `LUMIA_EXTRAS` if set, else `<workspace>/extras`.
pub fn extras_dir(manifest_dir: impl AsRef<Path>) -> PathBuf {
    if let Ok(p) = std::env::var("LUMIA_EXTRAS") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    workspace_root(manifest_dir).join("extras")
}

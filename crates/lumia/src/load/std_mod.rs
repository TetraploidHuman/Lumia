//! `std.*` and optional `extras.*` module loading + `@exports` validation.

use anyhow::{bail, Context, Result};
use lumia_syntax::{Import, ImportNames, Sym};
use rustc_hash::FxHashSet as HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn path_to_dotted(path: &[Sym]) -> String {
    path.iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

pub(super) fn is_std(path: &[Sym]) -> bool {
    path.first().map(|s| s.as_str() == "std").unwrap_or(false)
}

/// Optional domain modules under workspace `extras/` (not language std).
pub(super) fn is_extras(path: &[Sym]) -> bool {
    path.first()
        .map(|s| s.as_str() == "extras")
        .unwrap_or(false)
}

/// Resolve `std.<a>.<b>` → relative path under workspace `std/` (`a/b.lm`).
///
/// Discovery is filesystem-based: any `*.lm` under the bundled dir is importable
/// (no compile-time allowlist). Segment `..` / separators are rejected.
pub(super) fn std_module(path: &[Sym]) -> Result<PathBuf> {
    resolve_bundled_rel("std", path, &workspace_std_dir())
}

/// Resolve `extras.<name>` → relative path under workspace `extras/`.
pub(super) fn extras_module(path: &[Sym]) -> Result<PathBuf> {
    resolve_bundled_rel("extras", path, &workspace_extras_dir())
}

fn resolve_bundled_rel(kind: &str, path: &[Sym], dir: &Path) -> Result<PathBuf> {
    if path.first().map(|s| s.as_str()) != Some(kind) || path.len() < 2 {
        bail!("not a `{kind}.*` module path `{}`", path_to_dotted(path));
    }
    for seg in &path[1..] {
        if seg.is_empty()
            || *seg == "."
            || *seg == ".."
            || seg.contains('/')
            || seg.contains('\\')
            || seg.contains('\0')
        {
            bail!("invalid `{kind}` module path segment `{seg}`");
        }
    }
    let mut rel = PathBuf::new();
    let segs = &path[1..];
    for s in &segs[..segs.len() - 1] {
        rel.push(s.as_str());
    }
    rel.push(format!("{}.lm", segs.last().expect("len >= 2")));
    let file = dir.join(&rel);
    if !file.is_file() {
        let known = list_known_modules(kind, dir);
        bail!(
            "unknown {kind} module `{}` (known: {known})",
            path_to_dotted(path)
        );
    }
    Ok(rel)
}

fn list_known_modules(kind: &str, dir: &Path) -> String {
    let mut names = Vec::new();
    collect_lm_modules(dir, dir, &mut names);
    names.sort();
    if names.is_empty() {
        format!("(none under {})", dir.display())
    } else {
        names
            .into_iter()
            .map(|n| format!("{kind}.{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn collect_lm_modules(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.is_dir() {
            collect_lm_modules(root, &path, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("lm") {
            continue;
        }
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let mut parts = Vec::new();
        for c in rel.components() {
            let std::path::Component::Normal(os) = c else {
                continue;
            };
            let Some(s) = os.to_str() else {
                continue;
            };
            if let Some(stem) = s.strip_suffix(".lm") {
                parts.push(stem.to_string());
            } else {
                parts.push(s.to_string());
            }
        }
        if !parts.is_empty() {
            out.push(parts.join("."));
        }
    }
}

/// Export sets are read from module `@exports` lines — no hardcoded dual list.
pub(super) fn bundled_exports(path: &[Sym]) -> Result<Vec<String>> {
    let (dir, rel) = if is_std(path) {
        (workspace_std_dir(), std_module(path)?)
    } else if is_extras(path) {
        (workspace_extras_dir(), extras_module(path)?)
    } else {
        bail!("not a bundled module `{}`", path_to_dotted(path));
    };
    let file = dir.join(rel);
    let src = fs::read_to_string(&file).with_context(|| {
        format!(
            "read bundled module {} (expected at {})",
            path_to_dotted(path),
            file.display()
        )
    })?;
    parse_std_exports(&src).with_context(|| format!("parse @exports in {}", file.display()))
}

pub(super) fn workspace_std_dir() -> PathBuf {
    crate::paths::std_dir(env!("CARGO_MANIFEST_DIR"))
}

pub(super) fn workspace_extras_dir() -> PathBuf {
    crate::paths::extras_dir(env!("CARGO_MANIFEST_DIR"))
}

pub(super) fn parse_std_exports(src: &str) -> Result<Vec<String>> {
    match crate::exports::parse_exports_from_source(src) {
        None => bail!("missing `/// @exports …` line in bundled module source"),
        Some(names) if names.is_empty() => bail!("@exports list is empty"),
        Some(names) => Ok(names),
    }
}

pub(super) fn validate_bundled_import(imp: &Import) -> Result<()> {
    let exports = bundled_exports(&imp.path)?;
    let export_set: HashSet<&str> = exports.iter().map(|s| s.as_str()).collect();
    match &imp.names {
        // Visibility for `*` is filtered to `@exports` in `resolve` (FFI stays
        // inlined for wrapper callees but is not entry-visible).
        ImportNames::All => Ok(()),
        ImportNames::Single(n) => {
            if export_set.contains(n.name.as_str()) {
                Ok(())
            } else {
                bail!(
                    "`{}` is not exported by `{}` (exports: {})",
                    n.name,
                    path_to_dotted(&imp.path),
                    exports.join(", ")
                )
            }
        }
        ImportNames::Selective(names) => {
            for n in names {
                if !export_set.contains(n.name.as_str()) {
                    bail!(
                        "`{}` is not exported by `{}` (exports: {})",
                        n.name,
                        path_to_dotted(&imp.path),
                        exports.join(", ")
                    );
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn std_io_resolves_by_directory() {
        let rel = std_module(&[Sym::from("std"), Sym::from("io")]).expect("std.io");
        assert_eq!(rel, PathBuf::from("io.lm"));
    }

    #[test]
    fn unknown_std_lists_known() {
        let err = std_module(&[Sym::from("std"), Sym::from("nope")]).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unknown std module"), "{msg}");
        assert!(msg.contains("std.io"), "{msg}");
    }

    #[test]
    fn rejects_path_traversal_segment() {
        let err = std_module(&[Sym::from("std"), Sym::from(".."), Sym::from("io")]).unwrap_err();
        assert!(format!("{err}").contains("invalid"), "{err}");
    }
}

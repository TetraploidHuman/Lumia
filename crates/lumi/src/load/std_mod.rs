//! `lumi.*` standard library module loading and `@exports` validation.

use anyhow::{bail, Context, Result};
use lumi_syntax::{Import, ImportNames, Item, Span};
use rustc_hash::FxHashSet as HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::vis::item_name;

/// Submodules auto-imported into every entry module (Kotlin-style core stdlib).
/// Domain modules (`linalg`, `cn`, `efe`) use explicit `import` — their short
/// names (`add`, `mul`, …) would collide with user packages.
pub(crate) const LUMI_STD_SUBMODULES: &[&str] = &["io", "string", "option", "result"];

/// All known `lumi.<name>` modules (auto-import core + domain). Used by the loader
/// and LSP import-path completion — keep in sync with [`lumi_module`].
pub(crate) const KNOWN_LUMI_MODULES: &[&str] =
    &["io", "string", "option", "result", "linalg", "efe", "cn"];

/// Synthetic `import lumi.<mod>.*` for each known stdlib submodule.
pub(super) fn default_std_imports() -> Vec<Import> {
    LUMI_STD_SUBMODULES
        .iter()
        .map(|m| Import {
            path: vec!["lumi".into(), (*m).into()],
            names: ImportNames::All,
            span: Span::dummy(),
        })
        .collect()
}

/// Prepend default stdlib imports, then apply explicit user `lumi.*` imports
/// (selective imports add aliases without hiding other default exports).
pub(super) fn merge_std_imports(defaults: Vec<Import>, user: Vec<Import>) -> Vec<Import> {
    let mut out = defaults;
    out.extend(user);
    out
}

pub(super) fn is_lumi(path: &[String]) -> bool {
    path.first().map(|s| s.as_str() == "lumi").unwrap_or(false)
}

/// Resolve `lumi.<name>` → relative path under workspace `lumi/`.
pub(super) fn lumi_module(path: &[String]) -> Result<&'static str> {
    let key: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    match key.as_slice() {
        ["lumi", "io"] => Ok("io.lm"),
        ["lumi", "string"] => Ok("string.lm"),
        ["lumi", "option"] => Ok("option.lm"),
        ["lumi", "result"] => Ok("result.lm"),
        ["lumi", "linalg"] => Ok("linalg.lm"),
        ["lumi", "efe"] => Ok("efe.lm"),
        ["lumi", "cn"] => Ok("cn.lm"),
        _ => bail!(
            "unknown standard module `{}` (known: {})",
            path.join("."),
            KNOWN_LUMI_MODULES
                .iter()
                .map(|m| format!("lumi.{m}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Export sets are read from `lumi/<mod>.lm` `@exports` lines — no hardcoded dual list.
pub(crate) fn lumi_exports(path: &[String]) -> Result<Vec<String>> {
    let rel = lumi_module(path)?;
    let file = workspace_lumi_dir().join(rel);
    let src = fs::read_to_string(&file).with_context(|| {
        format!(
            "read standard module {} (expected at {})",
            path.join("."),
            file.display()
        )
    })?;
    parse_lumi_exports(&src).with_context(|| format!("parse @exports in {}", file.display()))
}

pub(super) fn workspace_lumi_dir() -> PathBuf {
    // crates/lumi -> workspace root
    lumi_abi::workspace_root(env!("CARGO_MANIFEST_DIR")).join("lumi")
}

pub(super) fn is_stdlib_module_name(name: &str) -> bool {
    LUMI_STD_SUBMODULES.contains(&name)
}

/// User modules get default std imports; stdlib sources under `lumi/` do not.
pub(super) fn wants_default_std_imports(path: &Path) -> bool {
    let std_dir = workspace_lumi_dir().canonicalize().ok();
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    !std_dir.is_some_and(|d| canon.starts_with(d))
}

pub(super) fn parse_lumi_exports(src: &str) -> Result<Vec<String>> {
    for line in src.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("///") else {
            continue;
        };
        let rest = rest.trim();
        let Some(list) = rest.strip_prefix("@exports") else {
            continue;
        };
        let list = list.trim().trim_start_matches(':').trim();
        let names: Vec<String> = list
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            bail!("@exports list is empty");
        }
        return Ok(names);
    }
    bail!("missing `/// @exports …` line in standard module source")
}

pub(super) fn is_synthetic_std_import(imp: &Import) -> bool {
    imp.span == Span::dummy()
}

/// Drop auto-imported std names that the entry module defines locally (Kotlin-style shadowing).
pub(super) fn drop_entry_shadowed(items: Vec<Item>, reserved: &HashSet<String>) -> Vec<Item> {
    if reserved.is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|it| item_name(it).is_none_or(|n| !reserved.contains(n)))
        .collect()
}

pub(super) fn validate_lumi_import(imp: &Import) -> Result<()> {
    let exports = lumi_exports(&imp.path)?;
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
                    imp.path.join("."),
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
                        imp.path.join("."),
                        exports.join(", ")
                    );
                }
            }
            Ok(())
        }
    }
}

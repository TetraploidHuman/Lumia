//! `lumi.*` standard library module loading and `@exports` validation.

use anyhow::{bail, Context, Result};
use lumi_syntax::{Import, ImportNames, Item, Span};
use rustc_hash::FxHashSet as HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::vis::item_name;

/// One known `lumi.<name>` module.
#[derive(Debug, Clone, Copy)]
struct LumiMod {
    name: &'static str,
    /// Relative path under workspace `lumi/`.
    file: &'static str,
    /// Auto-imported into every entry module (Kotlin-style core).
    auto_import: bool,
}

/// Single registry: auto-import core first, then domain modules that need an
/// explicit `import` (short names like `add`/`mul` would collide with user pkgs).
macro_rules! define_lumi_modules {
    (
        auto: [$(($an:literal, $af:literal)),* $(,)?];
        domain: [$(($dn:literal, $df:literal)),* $(,)?];
    ) => {
        const LUMI_MODULES: &[LumiMod] = &[
            $(LumiMod { name: $an, file: $af, auto_import: true },)*
            $(LumiMod { name: $dn, file: $df, auto_import: false },)*
        ];
        /// Submodules auto-imported into every entry module.
        pub(crate) const LUMI_STD_SUBMODULES: &[&str] = &[$($an),*];
        /// All known `lumi.<name>` modules (auto-import core + domain).
        pub(crate) const KNOWN_LUMI_MODULES: &[&str] = &[$($an,)* $($dn),*];
    };
}

define_lumi_modules! {
    auto: [
        ("io", "io.lm"),
        ("string", "string.lm"),
        ("option", "option.lm"),
        ("result", "result.lm"),
    ];
    domain: [
        ("linalg", "linalg.lm"),
        ("efe", "efe.lm"),
        ("cn", "cn.lm"),
    ];
}

fn find_lumi_mod(name: &str) -> Option<&'static LumiMod> {
    LUMI_MODULES.iter().find(|m| m.name == name)
}

/// Synthetic `import lumi.<mod>.*` for each auto-imported stdlib submodule.
pub(super) fn default_std_imports() -> Vec<Import> {
    LUMI_MODULES
        .iter()
        .filter(|m| m.auto_import)
        .map(|m| Import {
            path: vec!["lumi".into(), m.name.into()],
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

fn known_lumi_list() -> String {
    LUMI_MODULES
        .iter()
        .map(|m| format!("lumi.{}", m.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve `lumi.<name>` → relative path under workspace `lumi/`.
pub(super) fn lumi_module(path: &[String]) -> Result<&'static str> {
    match path {
        [root, name] if root.as_str() == "lumi" => {
            find_lumi_mod(name).map(|m| m.file).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown standard module `{}` (known: {})",
                    path.join("."),
                    known_lumi_list()
                )
            })
        }
        _ => bail!(
            "unknown standard module `{}` (known: {})",
            path.join("."),
            known_lumi_list()
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
    find_lumi_mod(name).is_some_and(|m| m.auto_import)
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

fn ensure_exported(
    name: &str,
    path: &[String],
    exports: &[String],
    export_set: &HashSet<&str>,
) -> Result<()> {
    if export_set.contains(name) {
        Ok(())
    } else {
        bail!(
            "`{}` is not exported by `{}` (exports: {})",
            name,
            path.join("."),
            exports.join(", ")
        )
    }
}

pub(super) fn validate_lumi_import(imp: &Import) -> Result<()> {
    let exports = lumi_exports(&imp.path)?;
    let export_set: HashSet<&str> = exports.iter().map(|s| s.as_str()).collect();
    match &imp.names {
        // Visibility for `*` is filtered to `@exports` in `resolve` (FFI stays
        // inlined for wrapper callees but is not entry-visible).
        ImportNames::All => Ok(()),
        ImportNames::Single(n) => {
            ensure_exported(n.name.as_str(), &imp.path, &exports, &export_set)
        }
        ImportNames::Selective(names) => {
            for n in names {
                ensure_exported(n.name.as_str(), &imp.path, &exports, &export_set)?;
            }
            Ok(())
        }
    }
}

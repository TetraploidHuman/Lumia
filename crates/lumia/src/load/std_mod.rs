//! `std.*` module loading and `@exports` validation.

use anyhow::{bail, Context, Result};
use lumia_syntax::{Import, ImportNames};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::fs;
use std::path::PathBuf;

pub(super) fn is_std(path: &[String]) -> bool {
    path.first().map(|s| s.as_str() == "std").unwrap_or(false)
}

/// How a `std.*` module is provided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StdKind {
    /// Bodies are compiler builtins; `std/<mod>.lm` only lists `@exports`.
    Builtin,
    /// Real Lumia source under `std/<mod>.lm`, inlined like a user module.
    Source,
}

/// Resolve `std.<name>` → (`rel` under workspace `std/`, kind).
pub(super) fn std_module(path: &[String]) -> Result<(&'static str, StdKind)> {
    let key: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    match key.as_slice() {
        ["std", "io"] => Ok(("io.lm", StdKind::Builtin)),
        ["std", "string"] => Ok(("string.lm", StdKind::Builtin)),
        ["std", "option"] => Ok(("option.lm", StdKind::Source)),
        ["std", "result"] => Ok(("result.lm", StdKind::Source)),
        _ => bail!(
            "unknown standard module `{}` (known: std.io, std.string, std.option, std.result)",
            path.join(".")
        ),
    }
}

pub(super) fn is_std_builtin(path: &[String]) -> bool {
    matches!(std_module(path), Ok((_, StdKind::Builtin)))
}

/// Export sets are read from `std/<mod>.lm` `@exports` lines — no hardcoded dual list.
pub(super) fn std_exports(path: &[String]) -> Result<Vec<String>> {
    let (rel, _) = std_module(path)?;
    let file = workspace_std_dir().join(rel);
    let src = fs::read_to_string(&file).with_context(|| {
        format!(
            "read standard module {} (expected at {})",
            path.join("."),
            file.display()
        )
    })?;
    parse_std_exports(&src).with_context(|| format!("parse @exports in {}", file.display()))
}

pub(super) fn workspace_std_dir() -> PathBuf {
    // crates/lumia -> workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("std")
}

pub(super) fn parse_std_exports(src: &str) -> Result<Vec<String>> {
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

pub(super) fn validate_std_import(imp: &Import) -> Result<()> {
    let exports = std_exports(&imp.path)?;
    let export_set: HashSet<&str> = exports.iter().map(|s| s.as_str()).collect();
    match &imp.names {
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

/// Register `as` aliases for std builtins (e.g. `println as log`).
pub(super) fn collect_std_aliases(imp: &Import, out: &mut HashMap<String, String>) -> Result<()> {
    let pairs: Vec<(&str, &str)> = match &imp.names {
        ImportNames::All => return Ok(()),
        ImportNames::Single(n) => {
            if let Some(a) = &n.alias {
                vec![(a.as_str(), n.name.as_str())]
            } else {
                return Ok(());
            }
        }
        ImportNames::Selective(ns) => ns
            .iter()
            .filter_map(|n| n.alias.as_ref().map(|a| (a.as_str(), n.name.as_str())))
            .collect(),
    };
    for (alias, canon) in pairs {
        if alias == canon {
            continue;
        }
        if let Some(prev) = out.insert(alias.to_string(), canon.to_string()) {
            if prev != canon {
                bail!("import alias `{alias}` conflict (`{prev}` vs `{canon}`)");
            }
        }
    }
    Ok(())
}

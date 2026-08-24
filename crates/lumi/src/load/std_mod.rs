//! `lumi.*` standard library module loading and `@exports` validation.

use anyhow::{bail, Context, Result};
use lumi_syntax::{Import, ImportNames};
use rustc_hash::FxHashSet as HashSet;
use std::fs;
use std::path::PathBuf;

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
            "unknown standard module `{}` (known: lumi.io, lumi.string, lumi.option, lumi.result, lumi.linalg, lumi.efe, lumi.cn)",
            path.join(".")
        ),
    }
}

/// Export sets are read from `lumi/<mod>.lm` `@exports` lines — no hardcoded dual list.
pub(super) fn lumi_exports(path: &[String]) -> Result<Vec<String>> {
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

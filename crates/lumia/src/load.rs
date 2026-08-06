//! Multi-file module loading: resolve non-`std` imports relative to the entry file.

use anyhow::{bail, Context, Result};
use lumia_syntax::{parse_module, Import, ImportNames, Item, Module};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn load_program(entry: &Path) -> Result<Module> {
    let entry = entry
        .canonicalize()
        .with_context(|| format!("canonicalize {}", entry.display()))?;
    let package_root = entry
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut visited = HashSet::new();
    load_module_file(&entry, &package_root, &mut visited)
}

fn is_std(path: &[String]) -> bool {
    path.first().map(|s| s.as_str() == "std").unwrap_or(false)
}

/// Candidate files for an import path segment list, e.g. `["pkg","math"]`.
fn path_candidates(base: &Path, rel: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if rel.is_empty() {
        return out;
    }
    // pkg/math.lumia
    out.push(base.join(rel.join("/")).with_extension("lumia"));
    // pkg/math/mod.lumia
    let mut mod_dir = base.join(rel.join("/"));
    mod_dir.push("mod.lumia");
    out.push(mod_dir);
    // pkg.math.lumia (flat dotted file)
    out.push(base.join(format!("{}.lumia", rel.join("."))));
    out
}

fn resolve_import_file(
    importer_dir: &Path,
    package_root: &Path,
    imp: &Import,
) -> Result<PathBuf> {
    let rel: Vec<&str> = match &imp.names {
        ImportNames::Single(_) if imp.path.is_empty() => {
            let ImportNames::Single(name) = &imp.names else {
                unreachable!();
            };
            let rel = [name.as_str()];
            for base in [importer_dir, package_root] {
                for cand in path_candidates(base, &rel) {
                    if cand.is_file() {
                        return Ok(cand);
                    }
                }
            }
            bail!(
                "cannot find module `{name}` (tried under {} and {})",
                importer_dir.display(),
                package_root.display()
            );
        }
        _ => imp.path.iter().map(|s| s.as_str()).collect(),
    };
    if rel.is_empty() {
        bail!("import path is empty");
    }
    let mut tried = Vec::new();
    for base in [importer_dir, package_root] {
        for cand in path_candidates(base, &rel) {
            if cand.is_file() {
                return Ok(cand);
            }
            tried.push(cand.display().to_string());
        }
    }
    bail!(
        "cannot find module for import path `{}` (tried {})",
        rel.join("."),
        tried.join(", ")
    )
}

fn filter_items(items: Vec<Item>, names: &ImportNames) -> Result<Vec<Item>> {
    let (privs, pubs): (Vec<_>, Vec<_>) = items.into_iter().partition(item_is_priv);
    match names {
        ImportNames::All => {
            let mut out = pubs;
            out.extend(privs);
            Ok(out)
        }
        ImportNames::Single(name) => {
            if privs.iter().any(|it| item_name(it) == Some(name.as_str())) {
                bail!("cannot import private `{name}`");
            }
            if !pubs.iter().any(|it| item_name(it) == Some(name.as_str())) {
                bail!("module has no public `{name}`");
            }
            // MVP: inlining pulls the whole module so callees resolve.
            let mut out = pubs;
            out.extend(privs);
            Ok(out)
        }
        ImportNames::Selective(ns) => {
            for n in ns {
                if privs.iter().any(|it| item_name(it) == Some(n.as_str())) {
                    bail!("cannot import private `{n}`");
                }
                if !pubs.iter().any(|it| item_name(it) == Some(n.as_str())) {
                    bail!("module has no public `{n}`");
                }
            }
            let mut out = pubs;
            out.extend(privs);
            Ok(out)
        }
    }
}

fn item_name(it: &Item) -> Option<&str> {
    match it {
        Item::Val(v) => Some(v.name.as_str()),
        Item::Type(t) => Some(t.name.as_str()),
    }
}

fn item_is_priv(it: &Item) -> bool {
    match it {
        Item::Val(v) => v.is_priv,
        Item::Type(t) => t.is_priv,
    }
}

fn load_module_file(
    path: &Path,
    package_root: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<Module> {
    if !visited.insert(path.to_path_buf()) {
        bail!("cyclic import involving {}", path.display());
    }
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut m = parse_module(&src)
        .map_err(|e| anyhow::anyhow!("parse {}: {} @ {:?}", path.display(), e.message, e.span))?;
    let importer_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut imported_items = Vec::new();
    for imp in &m.imports {
        if is_std(&imp.path) {
            continue;
        }
        let file = resolve_import_file(&importer_dir, package_root, imp)?;
        let dep = load_module_file(&file, package_root, visited)?;
        imported_items.extend(filter_items(dep.items, &imp.names)?);
    }

    // Keep only std imports (decorative / builtins); user modules are inlined.
    m.imports.retain(|i| is_std(&i.path));
    imported_items.append(&mut m.items);
    m.items = imported_items;
    Ok(m)
}

//! Multi-file module loading: resolve non-`std` imports relative to the entry file.
//!
//! Entire dependency modules are inlined so private callees of public APIs remain
//! linkable, but [`lumia_ty::NameVisibility`] ensures `priv` names cannot be
//! referenced from the entry module's own code.

use crate::vis::{extend_visibility, import_visible_names, item_is_priv, item_name};
use anyhow::{bail, Context, Result};
use lumia_syntax::{
    format_diagnostic, parse_module, stamp_module, Import, ImportNames, Item, Module,
};
use lumia_ty::NameVisibility;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// One source file in the compilation unit.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub src: String,
}

/// Entry module plus SourceMap (for located diagnostics across imports).
#[derive(Debug, Clone)]
pub struct LoadedProgram {
    pub files: Vec<SourceFile>,
    pub module: Module,
    /// Linker flags from `Lumia.toml` `package.link`.
    pub link_args: Vec<String>,
    /// Cross-file name visibility for type checking.
    pub visibility: NameVisibility,
}

impl LoadedProgram {
    pub fn file(&self, id: u32) -> &SourceFile {
        &self.files[id as usize]
    }
}

pub fn load_program(entry: &Path) -> Result<LoadedProgram> {
    load_program_with_overlays(entry, &HashMap::new())
}

/// Load with in-memory overlays (URI/path → buffer), for LSP unsaved edits.
pub fn load_program_with_overlays(
    entry: &Path,
    overlays: &HashMap<PathBuf, String>,
) -> Result<LoadedProgram> {
    let entry = if entry.exists() {
        entry
            .canonicalize()
            .with_context(|| format!("canonicalize {}", entry.display()))?
    } else {
        if entry.is_absolute() {
            entry.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(entry)
        }
    };
    let package_root = entry
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut search_roots = vec![package_root.clone()];
    let mut link_args = Vec::new();
    if let Some(manifest_path) = crate::pkg::find_manifest(&entry) {
        let m = crate::pkg::load_manifest(&manifest_path)
            .with_context(|| format!("load {}", manifest_path.display()))?;
        let lock_path = manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("Lumia.lock");
        if !m.dependencies.is_empty() && !lock_path.is_file() {
            bail!(
                "dependencies declared in {} but {} is missing (run `lumia pkg lock`)",
                manifest_path.display(),
                lock_path.display()
            );
        }
        if lock_path.is_file() {
            let lock = crate::pkg::load_lockfile(&lock_path)?;
            crate::pkg::verify_lockfile(&manifest_path, &m, &lock)?;
        }
        link_args = crate::pkg::collect_link_args(&manifest_path, &m)?;
        let roots = crate::pkg::dependency_roots(&manifest_path, &m)?;
        for r in roots {
            if !search_roots.iter().any(|x| x == &r) {
                search_roots.push(r);
            }
        }
    }
    let overlay_by_canon = normalize_overlays(overlays);
    let mut visited = HashSet::new();
    let mut files = Vec::new();
    let mut visibility = NameVisibility::default();
    let module = load_module_file(
        &entry,
        &search_roots,
        &overlay_by_canon,
        &mut visited,
        &mut files,
        &mut visibility,
        true,
    )?;
    let entry_file = 0; // entry is always stamped as the first file pushed
    visibility.entry_file = entry_file;
    Ok(LoadedProgram {
        files,
        module,
        link_args,
        visibility,
    })
}

fn normalize_overlays(overlays: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    for (p, src) in overlays {
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        out.insert(key, src.clone());
    }
    out
}

fn is_std(path: &[String]) -> bool {
    path.first().map(|s| s.as_str() == "std").unwrap_or(false)
}

/// Compiler-provided `std.*` modules (implemented as builtins / prelude).
/// Export sets are read from `std/<mod>.lumia` `@exports` lines — no hardcoded dual list.
fn std_exports(path: &[String]) -> Result<Vec<String>> {
    let key: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
    let rel = match key.as_slice() {
        ["std", "io"] => "io.lumia",
        ["std", "string"] => "string.lumia",
        _ => {
            bail!(
                "unknown standard module `{}` (known: std.io, std.string)",
                path.join(".")
            );
        }
    };
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

fn workspace_std_dir() -> PathBuf {
    // crates/lumia -> workspace root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("std")
}

fn parse_std_exports(src: &str) -> Result<Vec<String>> {
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

fn validate_std_import(imp: &Import) -> Result<()> {
    let exports = std_exports(&imp.path)?;
    let export_set: HashSet<&str> = exports.iter().map(|s| s.as_str()).collect();
    match &imp.names {
        ImportNames::All => Ok(()),
        ImportNames::Single(name) => {
            if export_set.contains(name.as_str()) {
                Ok(())
            } else {
                bail!(
                    "`{name}` is not exported by `{}` (exports: {})",
                    imp.path.join("."),
                    exports.join(", ")
                )
            }
        }
        ImportNames::Selective(names) => {
            for n in names {
                if !export_set.contains(n.as_str()) {
                    bail!(
                        "`{n}` is not exported by `{}` (exports: {})",
                        imp.path.join("."),
                        exports.join(", ")
                    );
                }
            }
            Ok(())
        }
    }
}

fn path_candidates(base: &Path, rel: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if rel.is_empty() {
        return out;
    }
    out.push(base.join(rel.join("/")).with_extension("lumia"));
    let mut mod_dir = base.join(rel.join("/"));
    mod_dir.push("mod.lumia");
    out.push(mod_dir);
    out.push(base.join(format!("{}.lumia", rel.join("."))));
    out
}

fn resolve_import_file(
    importer_dir: &Path,
    search_roots: &[PathBuf],
    imp: &Import,
) -> Result<PathBuf> {
    let mut bases: Vec<&Path> = vec![importer_dir];
    for r in search_roots {
        bases.push(r.as_path());
    }
    let rel: Vec<&str> = match &imp.names {
        ImportNames::Single(_) if imp.path.is_empty() => {
            let ImportNames::Single(name) = &imp.names else {
                unreachable!();
            };
            let rel = [name.as_str()];
            for base in &bases {
                for cand in path_candidates(base, &rel) {
                    if cand.is_file() {
                        return Ok(cand);
                    }
                }
            }
            bail!(
                "cannot find module `{name}` (tried under {})",
                bases
                    .iter()
                    .map(|b| b.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        _ => imp.path.iter().map(|s| s.as_str()).collect(),
    };
    if rel.is_empty() {
        bail!("import path is empty");
    }
    let mut tried = Vec::new();
    for base in &bases {
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
            // Keep whole module for private callees of public APIs; visibility
            // is enforced separately via NameVisibility.
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

fn load_module_file(
    path: &Path,
    search_roots: &[PathBuf],
    overlays: &HashMap<PathBuf, String>,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<SourceFile>,
    visibility: &mut NameVisibility,
    is_entry: bool,
) -> Result<Module> {
    if !visited.insert(path.to_path_buf()) {
        bail!("cyclic import involving {}", path.display());
    }
    let src = if let Some(buf) = overlays.get(path) {
        buf.clone()
    } else {
        fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    };
    let file_id = files.len() as u32;
    files.push(SourceFile {
        path: path.to_path_buf(),
        src: src.clone(),
    });
    let mut m = parse_module(&src).map_err(|e| {
        anyhow::anyhow!(format_diagnostic(
            &path_label(path),
            &src,
            e.span.with_file(file_id),
            "parse",
            &e.message,
        ))
    })?;
    stamp_module(&mut m, file_id);

    let importer_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let entry_file = if is_entry { file_id } else { visibility.entry_file };
    if is_entry {
        visibility.entry_file = file_id;
    }
    let _ = entry_file;

    let mut imported_items = Vec::new();
    for imp in &m.imports {
        if is_std(&imp.path) {
            validate_std_import(imp)?;
            continue;
        }
        let file = resolve_import_file(&importer_dir, search_roots, imp)?;
        let dep = load_module_file(
            &file,
            search_roots,
            overlays,
            visited,
            files,
            visibility,
            false,
        )?;
        let visible = import_visible_names(&dep.items, &imp.names);
        // Only the entry module's imports expand the user-facing scope.
        if is_entry {
            extend_visibility(visibility, &dep.items, &visible);
        } else {
            // Nested deps: record origins only (no new entry-visible names).
            let empty = HashSet::new();
            extend_visibility(visibility, &dep.items, &empty);
        }
        imported_items.extend(filter_items(dep.items, &imp.names)?);
    }

    m.imports.retain(|i| is_std(&i.path));
    // Record this file's declarations (entry or dep). Entry names are visible
    // via same-file origin; deps rely on import_visible_names above.
    let local_visible: HashSet<String> = if is_entry {
        m.items
            .iter()
            .filter_map(item_name)
            .map(|s| s.to_string())
            .collect()
    } else {
        HashSet::new()
    };
    extend_visibility(visibility, &m.items, &local_visible);

    imported_items.append(&mut m.items);
    m.items = imported_items;
    Ok(m)
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

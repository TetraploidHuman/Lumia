//! Multi-file module loading: resolve non-`std` imports relative to the entry file.
//!
//! Entire dependency modules are inlined so private callees of public APIs remain
//! linkable, but [`lumia_ty::NameVisibility`] ensures `priv` names cannot be
//! referenced from the entry module's own code.

mod aliases;
mod resolve;
mod std_mod;

use crate::vis::item_name;
use anyhow::{bail, Context, Result};
use lumia_syntax::{Item, Module};
use lumia_ty::NameVisibility;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::path::{Path, PathBuf};

use aliases::rewrite_builtin_alias_idents;
use resolve::{load_module_file, path_label};

pub(super) fn item_file_id(it: &Item) -> u32 {
    match it {
        Item::Val(v) => v.span.file,
        Item::Type(t) => t.span.file,
        Item::Foreign(f) => f.span.file,
        Item::Trait(t) => t.span.file,
        Item::Instance(i) => i.span.file,
    }
}

/// Reject silently-overwritten top-level names after import inlining.
pub(super) fn check_no_duplicate_toplevel(items: &[Item], files: &[SourceFile]) -> Result<()> {
    let mut seen: HashMap<&str, u32> = HashMap::default();
    for it in items {
        let Some(name) = item_name(it) else {
            continue;
        };
        let file = item_file_id(it);
        if let Some(&prev) = seen.get(name) {
            if prev == file {
                bail!("duplicate top-level name `{name}`");
            }
            let a = files
                .get(prev as usize)
                .map(|f| path_label(&f.path))
                .unwrap_or_else(|| format!("file#{prev}"));
            let b = files
                .get(file as usize)
                .map(|f| path_label(&f.path))
                .unwrap_or_else(|| format!("file#{file}"));
            bail!("duplicate top-level name `{name}` in `{a}` and `{b}`");
        }
        seen.insert(name, file);
    }
    Ok(())
}

/// Append items, skipping `(file, name)` pairs already present (diamond imports).
pub(super) fn append_items_unique(dst: &mut Vec<Item>, src: Vec<Item>) {
    let mut have: HashSet<(u32, String)> = dst
        .iter()
        .filter_map(|it| item_name(it).map(|n| (item_file_id(it), n.to_string())))
        .collect();
    for it in src {
        if let Some(name) = item_name(&it) {
            let key = (item_file_id(&it), name.to_string());
            if !have.insert(key) {
                continue;
            }
        }
        dst.push(it);
    }
}

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
    /// From `Lumia.toml` `package.trust_foreign_pure`.
    pub trust_foreign_pure: bool,
    /// Cross-file name visibility for type checking.
    pub visibility: NameVisibility,
}

impl LoadedProgram {
    pub fn file(&self, id: u32) -> &SourceFile {
        &self.files[id as usize]
    }
}

pub fn load_program(entry: &Path) -> Result<LoadedProgram> {
    load_program_with_overlays(entry, &HashMap::default())
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
    let mut trust_foreign_pure = false;
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
        trust_foreign_pure = m.package.trust_foreign_pure;
        let roots = crate::pkg::dependency_roots(&manifest_path, &m)?;
        for r in roots {
            if !search_roots.iter().any(|x| x == &r) {
                search_roots.push(r);
            }
        }
    }
    let overlay_by_canon = normalize_overlays(overlays);
    let mut stack = HashSet::default();
    let mut done = HashMap::default();
    let mut files = Vec::new();
    let mut visibility = NameVisibility::default();
    let module = load_module_file(
        &entry,
        &search_roots,
        &overlay_by_canon,
        &mut stack,
        &mut done,
        &mut files,
        &mut visibility,
        true,
    )?;
    let entry_file = 0; // entry is always stamped as the first file pushed
    visibility.entry_file = entry_file;
    let mut module = module;
    rewrite_builtin_alias_idents(&mut module, &visibility.builtin_aliases, entry_file);
    Ok(LoadedProgram {
        files,
        module,
        link_args,
        trust_foreign_pure,
        visibility,
    })
}

pub(super) fn normalize_overlays(overlays: &HashMap<PathBuf, String>) -> HashMap<PathBuf, String> {
    let mut out = HashMap::default();
    for (p, src) in overlays {
        let key = p.canonicalize().unwrap_or_else(|_| p.clone());
        out.insert(key, src.clone());
    }
    out
}

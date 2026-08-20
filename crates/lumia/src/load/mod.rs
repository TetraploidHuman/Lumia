//! Multi-file module loading: resolve non-`std` imports relative to the entry file.
//!
//! Entire dependency modules are inlined so private callees of public APIs remain
//! linkable, but [`lumia_ty::NameVisibility`] ensures `priv` names cannot be
//! referenced from the entry module's own code.

mod resolve;
mod std_mod;

use crate::vis::{item_file, item_name};
use anyhow::{bail, Context, Result};
use lumia_syntax::{Item, Module};
use lumia_ty::NameVisibility;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::path::{Path, PathBuf};

use resolve::load_module_file;

pub use resolve::path_label;

use std::sync::Once;

static TRUST_FOREIGN_PURE_WARNED: Once = Once::new();

/// Reject silently-overwritten top-level names after import inlining.
pub(super) fn check_no_duplicate_toplevel(items: &[Item], files: &[SourceFile]) -> Result<()> {
    let mut seen: HashMap<&str, u32> = HashMap::default();
    for it in items {
        let Some(name) = item_name(it) else {
            continue;
        };
        let file = item_file(it);
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
        .filter_map(|it| item_name(it).map(|n| (item_file(it), n.to_string())))
        .collect();
    for it in src {
        if let Some(name) = item_name(&it) {
            let key = (item_file(&it), name.to_string());
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

/// Conventional package entry paths relative to the package root (manifest dir).
/// Shared by LSP/`resolve_ide_entry` and editor Run actions — keep in sync with
/// IDEA `LumiaPaths.resolveProjectEntry`.
pub const PACKAGE_ENTRY_RELS: &[&str] = &["Main.lm", "main.lm", "src/Main.lm", "src/main.lm"];

/// Prefer package `Main.lm` / `main.lm` / `src/…` when an IDE analyzes a non-entry file.
///
/// CLI `lumia check path` still uses `path` as the entry. The language server
/// redirects so editing an imported library still sees the package graph (and
/// does not false-green while `Main` is broken). Returns `path` when there is
/// no manifest, no conventional entry, or `path` is already that entry.
pub fn resolve_ide_entry(path: &Path) -> PathBuf {
    let path_abs = if path.exists() {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let Some(manifest) = crate::pkg::find_manifest(&path_abs) else {
        return path_abs;
    };
    let Some(root) = manifest.parent() else {
        return path_abs;
    };
    let entry = PACKAGE_ENTRY_RELS
        .iter()
        .map(|n| root.join(n))
        .find(|p| p.is_file());
    let Some(entry) = entry else {
        return path_abs;
    };
    let entry = entry.canonicalize().unwrap_or(entry);
    if paths_same_file(&entry, &path_abs) {
        return path_abs;
    }
    if !path_under_root(&path_abs, root) {
        return path_abs;
    }
    entry
}

/// Whether `path` appears in a loaded program's source map (exact or canonical).
pub fn path_in_loaded_files(files: &[SourceFile], path: &Path) -> bool {
    files.iter().any(|f| paths_same_file(&f.path, path))
}

fn paths_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn path_under_root(path: &Path, root: &Path) -> bool {
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let cand = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    cand.starts_with(&root)
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
    let mut search_roots = vec![package_root];
    let mut link_args = Vec::new();
    let mut trust_foreign_pure = false;
    if let Some(manifest_path) = crate::pkg::find_manifest(&entry) {
        // Add the manifest (package) root as a search root so that modules placed
        // in subdirectories can still import shared sibling modules that live
        // directly under the package root (e.g. examples/guide/ imports math.lm
        // from examples/).
        if let Some(manifest_dir) = manifest_path.parent() {
            let manifest_dir = manifest_dir
                .canonicalize()
                .unwrap_or_else(|_| manifest_dir.to_path_buf());
            if !search_roots.iter().any(|x| x == &manifest_dir) {
                search_roots.push(manifest_dir);
            }
        }
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
            // Lock-driven dep paths (content-pinned); keep entry dir for relative imports.
            for r in crate::pkg::dependency_roots_from_lock(&manifest_path, &lock)? {
                if !search_roots.iter().any(|x| x == &r) {
                    search_roots.push(r);
                }
            }
        } else {
            let roots = crate::pkg::dependency_roots(&manifest_path, &m)?;
            for r in roots {
                if !search_roots.iter().any(|x| x == &r) {
                    search_roots.push(r);
                }
            }
        }
        link_args = crate::pkg::collect_link_args(&manifest_path, &m)?;
        trust_foreign_pure = m.package.trust_foreign_pure;
        if trust_foreign_pure {
            // Once per process: LSP re-loads often and must not spam stderr.
            TRUST_FOREIGN_PURE_WARNED.call_once(|| {
                eprintln!(
                    "warning: package.trust_foreign_pure=true honors unverified `foreign \"C\" pure` \
                     (same trust surface as --trust-foreign-pure; override with --no-trust-foreign-pure)"
                );
            });
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

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use std::fs;

    #[test]
    #[cfg(unix)]
    fn overlay_symlink_key_hits_canonical_load() {
        let dir = std::env::temp_dir().join(format!("lumia_ov_sym_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = dir.join("link");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let real_lib = real.join("Lib.lm");
        fs::write(&real_lib, "module Lib\nval x = 1\n").unwrap();
        let real_main = real.join("Main.lm");
        fs::write(
            &real_main,
            "module Main\nimport Lib.{x}\nval main = x + 1\n",
        )
        .unwrap();
        let link_lib = link.join("Lib.lm");
        let mut overlays = HashMap::default();
        overlays.insert(link_lib, "module Lib\nval x = \"hi\"\n".into());
        let err = crate::check::check_program_with_overlays(&real_main, &overlays, false, None)
            .expect_err("overlay should make x a String");
        let msg = match &err {
            crate::check::OverlayCheckError::Analyze { err, .. } => err.message().to_string(),
            crate::check::OverlayCheckError::Load(m) => m.clone(),
        };
        assert!(
            msg.contains("mismatch") || msg.contains("String") || msg.contains("Int"),
            "expected type error from overlay String, got {msg}"
        );
        crate::check::check_program_with_overlays(&real_main, &HashMap::default(), false, None)
            .expect("disk sources ok");
    }

    #[test]
    fn overlay_dotdot_key_hits_canonical_load() {
        let dir = std::env::temp_dir().join(format!("lumia_ov_dd_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let nest = dir.join("a").join("b");
        fs::create_dir_all(&nest).unwrap();
        let lib = nest.join("Lib.lm");
        fs::write(&lib, "module Lib\nval x = 1\n").unwrap();
        let main = nest.join("Main.lm");
        fs::write(&main, "module Main\nimport Lib.{x}\nval main = x + 1\n").unwrap();
        // Non-canonical overlay key: .../a/b/../b/Lib.lm
        let weird = nest.join("..").join("b").join("Lib.lm");
        let mut overlays = HashMap::default();
        overlays.insert(weird, "module Lib\nval x = \"hi\"\n".into());
        let err = crate::check::check_program_with_overlays(&main, &overlays, false, None)
            .expect_err("dotdot overlay key must apply");
        let msg = match &err {
            crate::check::OverlayCheckError::Analyze { err, .. } => err.message().to_string(),
            crate::check::OverlayCheckError::Load(m) => m.clone(),
        };
        assert!(
            msg.contains("mismatch") || msg.contains("String") || msg.contains("Int"),
            "expected type error from overlay, got {msg}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn overlay_real_key_entry_via_symlink() {
        let dir = std::env::temp_dir().join(format!("lumia_ov_inv_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = dir.join("link");
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        fs::write(real.join("Lib.lm"), "module Lib\nval x = 1\n").unwrap();
        fs::write(
            real.join("Main.lm"),
            "module Main\nimport Lib.{x}\nval main = x + 1\n",
        )
        .unwrap();
        let mut overlays = HashMap::default();
        overlays.insert(real.join("Lib.lm"), "module Lib\nval x = \"hi\"\n".into());
        let link_main = link.join("Main.lm");
        let err = crate::check::check_program_with_overlays(&link_main, &overlays, false, None)
            .expect_err("real-path overlay must apply when entry is symlink");
        let msg = match &err {
            crate::check::OverlayCheckError::Analyze { err, .. } => err.message().to_string(),
            crate::check::OverlayCheckError::Load(m) => m.clone(),
        };
        assert!(
            msg.contains("mismatch") || msg.contains("String") || msg.contains("Int"),
            "expected type error from overlay, got {msg}"
        );
    }

    #[test]
    fn overlay_relative_key_against_absolute_entry() {
        let dir = std::env::temp_dir().join(format!("lumia_ov_rel_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let lib = dir.join("Lib.lm");
        let main = dir.join("Main.lm");
        fs::write(&lib, "module Lib\nval x = 1\n").unwrap();
        fs::write(&main, "module Main\nimport Lib.{x}\nval main = x + 1\n").unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let mut overlays = HashMap::default();
        // Relative key as an editor might produce without absolutizing.
        overlays.insert(
            PathBuf::from("Lib.lm"),
            "module Lib\nval x = \"hi\"\n".into(),
        );
        let err =
            crate::check::check_program_with_overlays(Path::new("Main.lm"), &overlays, false, None);
        let _ = std::env::set_current_dir(&cwd);
        let err = err.expect_err("relative overlay key should apply");
        let msg = match &err {
            crate::check::OverlayCheckError::Analyze { err, .. } => err.message().to_string(),
            crate::check::OverlayCheckError::Load(m) => m.clone(),
        };
        assert!(
            msg.contains("mismatch") || msg.contains("String") || msg.contains("Int"),
            "expected type error from relative overlay, got {msg}"
        );
    }

    #[test]
    fn resolve_ide_entry_prefers_package_main() {
        let dir = std::env::temp_dir().join(format!("lumia_ide_entry_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Lumia.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let main = dir.join("Main.lm");
        let lib = dir.join("Lib.lm");
        fs::write(
            &main,
            "module Main\nimport Lib.{x}\nval main: Int = \"bad\"\n",
        )
        .unwrap();
        fs::write(&lib, "module Lib\nval x = 1\n").unwrap();
        let resolved = resolve_ide_entry(&lib);
        assert!(
            paths_same_file(&resolved, &main),
            "expected Main, got {}",
            resolved.display()
        );
        // Any package-local path prefers Main; LSP falls back if the buffer is
        // outside Main's import graph (`path_in_loaded_files`).
        let orphan = dir.join("Orphan.lm");
        fs::write(&orphan, "module Orphan\nval y = 2\n").unwrap();
        assert!(paths_same_file(&resolve_ide_entry(&orphan), &main));
        assert!(paths_same_file(&resolve_ide_entry(&main), &main));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_ide_entry_accepts_src_main() {
        let dir = std::env::temp_dir().join(format!("lumia_ide_src_main_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src = dir.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            dir.join("Lumia.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let main = src.join("main.lm");
        let lib = dir.join("Lib.lm");
        fs::write(&main, "module Main\nval main = 0\n").unwrap();
        fs::write(&lib, "module Lib\nval x = 1\n").unwrap();
        assert!(
            paths_same_file(&resolve_ide_entry(&lib), &main),
            "expected src/main.lm, got {}",
            resolve_ide_entry(&lib).display()
        );
        assert_eq!(
            PACKAGE_ENTRY_RELS,
            &["Main.lm", "main.lm", "src/Main.lm", "src/main.lm"]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ide_entry_load_surfaces_main_error_when_editing_lib() {
        let dir = std::env::temp_dir().join(format!("lumia_ide_vis_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Lumia.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let main = dir.join("Main.lm");
        let lib = dir.join("Lib.lm");
        fs::write(
            &main,
            "module Main\nimport Lib.{x}\nval main: Int = \"bad\"\n",
        )
        .unwrap();
        fs::write(&lib, "module Lib\nval x = 1\n").unwrap();
        // Lone Lib check stays green (CLI).
        crate::check::check_program(&lib, true, None).expect("Lib alone ok");
        // IDE entry = Main: Main's type error is visible, Lib is in the graph.
        let entry = resolve_ide_entry(&lib);
        let partial = crate::check::check_program_with_overlays_recovering(
            &entry,
            &HashMap::default(),
            true,
            None,
        )
        .expect("load");
        assert!(path_in_loaded_files(&partial.loaded.files, &lib));
        assert!(
            partial.diagnostics.iter().any(|d| {
                matches!(d.kind, crate::diag::DiagnosticKind::Type)
                    && (d.message.contains("mismatch")
                        || d.message.contains("String")
                        || d.message.contains("Int"))
            }),
            "expected Main type error via ide entry, got {:?}",
            partial.diagnostics
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ide_entry_falls_back_when_buffer_not_in_main_graph() {
        let dir = std::env::temp_dir().join(format!("lumia_ide_orphan_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Lumia.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let main = dir.join("Main.lm");
        let orphan = dir.join("Orphan.lm");
        fs::write(&main, "module Main\nval main = 0\n").unwrap();
        fs::write(&orphan, "module Orphan\nval y: Int = \"no\"\n").unwrap();
        let entry = resolve_ide_entry(&orphan);
        assert!(paths_same_file(&entry, &main));
        let via_main = crate::check::check_program_with_overlays_recovering(
            &entry,
            &HashMap::default(),
            true,
            None,
        )
        .expect("load Main");
        assert!(!path_in_loaded_files(&via_main.loaded.files, &orphan));
        // Fallback: analyze orphan as its own entry (LSP does this).
        let alone = crate::check::check_program_with_overlays_recovering(
            &orphan,
            &HashMap::default(),
            true,
            None,
        )
        .expect("load orphan");
        assert!(
            alone
                .diagnostics
                .iter()
                .any(|d| matches!(d.kind, crate::diag::DiagnosticKind::Type)),
            "orphan type error must surface on fallback, got {:?}",
            alone.diagnostics
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

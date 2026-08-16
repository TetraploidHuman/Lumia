//! Import path resolution and recursive module loading.

use super::std_mod::{
    bundled_exports, extras_module, is_extras, is_std, std_module, validate_bundled_import,
    workspace_extras_dir, workspace_std_dir,
};
use super::{append_items_unique, check_no_duplicate_toplevel, SourceFile};
use crate::vis::{
    apply_import_aliases, extend_visibility, import_visible_names, item_is_priv, item_name,
};
use anyhow::{bail, Context, Result};
use lumia_syntax::{
    format_diagnostic_files, parse_module, stamp_module, Import, ImportNames, Item, Module,
};
use lumia_ty::NameVisibility;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn path_candidates(base: &Path, rel: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if rel.is_empty() {
        return out;
    }
    out.push(base.join(rel.join("/")).with_extension("lm"));
    let mut mod_dir = base.join(rel.join("/"));
    mod_dir.push("mod.lm");
    out.push(mod_dir);
    out.push(base.join(format!("{}.lm", rel.join("."))));
    out
}

pub(super) fn resolve_import_file(
    importer_dir: &Path,
    search_roots: &[PathBuf],
    imp: &Import,
) -> Result<PathBuf> {
    let mut bases: Vec<&Path> = vec![importer_dir];
    for r in search_roots {
        bases.push(r.as_path());
    }
    let rel: Vec<&str> = match &imp.names {
        ImportNames::Single(n) if imp.path.is_empty() => {
            let rel = [n.name.as_str()];
            for base in &bases {
                for cand in path_candidates(base, &rel) {
                    if cand.is_file() {
                        return Ok(cand);
                    }
                }
            }
            bail!(
                "cannot find module `{}` (tried under {})",
                n.name,
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

pub(super) fn filter_items(items: Vec<Item>, names: &ImportNames) -> Result<Vec<Item>> {
    // Preserve source order: partitioning pubs-then-privs broke sequential
    // binding of `priv val` constants used by later public vals (Fun names
    // are pre-bound in ty, so that case still worked).
    match names {
        ImportNames::All => Ok(items),
        ImportNames::Single(n) => {
            if items
                .iter()
                .any(|it| item_is_priv(it) && item_name(it) == Some(n.name.as_str()))
            {
                bail!("cannot import private `{}`", n.name);
            }
            if !items
                .iter()
                .any(|it| !item_is_priv(it) && item_name(it) == Some(n.name.as_str()))
            {
                bail!("module has no public `{}`", n.name);
            }
            // Keep whole module for private callees of public APIs; visibility
            // is enforced separately via NameVisibility.
            Ok(apply_import_aliases(items, names))
        }
        ImportNames::Selective(ns) => {
            for n in ns {
                if items
                    .iter()
                    .any(|it| item_is_priv(it) && item_name(it) == Some(n.name.as_str()))
                {
                    bail!("cannot import private `{}`", n.name);
                }
                if !items
                    .iter()
                    .any(|it| !item_is_priv(it) && item_name(it) == Some(n.name.as_str()))
                {
                    bail!("module has no public `{}`", n.name);
                }
            }
            Ok(apply_import_aliases(items, names))
        }
    }
}

pub(super) fn load_module_file(
    path: &Path,
    search_roots: &[PathBuf],
    overlays: &HashMap<PathBuf, String>,
    stack: &mut HashSet<PathBuf>,
    done: &mut HashMap<PathBuf, Module>,
    files: &mut Vec<SourceFile>,
    visibility: &mut NameVisibility,
    is_entry: bool,
) -> Result<Module> {
    let path_key = path.to_path_buf();
    if let Some(cached) = done.get(&path_key) {
        return Ok(cached.clone());
    }
    if !stack.insert(path_key.clone()) {
        bail!("cyclic import involving {}", path.display());
    }
    let result = load_module_file_uncached(
        path,
        &path_key,
        search_roots,
        overlays,
        stack,
        done,
        files,
        visibility,
        is_entry,
    );
    stack.remove(&path_key);
    result
}

pub(super) fn load_module_file_uncached(
    path: &Path,
    path_key: &Path,
    search_roots: &[PathBuf],
    overlays: &HashMap<PathBuf, String>,
    stack: &mut HashSet<PathBuf>,
    done: &mut HashMap<PathBuf, Module>,
    files: &mut Vec<SourceFile>,
    visibility: &mut NameVisibility,
    is_entry: bool,
) -> Result<Module> {
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
        let labels: Vec<String> = files.iter().map(|f| path_label(&f.path)).collect();
        let table: Vec<(&str, &str)> = labels
            .iter()
            .zip(files.iter())
            .map(|(lab, f)| (lab.as_str(), f.src.as_str()))
            .collect();
        anyhow::anyhow!(format_diagnostic_files(
            &table,
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

    if is_entry {
        visibility.entry_file = file_id;
    }

    let mut imported_items = Vec::new();
    for imp in &m.imports {
        if is_std(&imp.path) || is_extras(&imp.path) {
            validate_bundled_import(imp)?;
            let (dir, rel) = if is_std(&imp.path) {
                (workspace_std_dir(), std_module(&imp.path)?)
            } else {
                (workspace_extras_dir(), extras_module(&imp.path)?)
            };
            let file = dir.join(rel);
            let file = file.canonicalize().unwrap_or(file);
            let dep = load_module_file(
                &file,
                search_roots,
                overlays,
                stack,
                done,
                files,
                visibility,
                false,
            )?;
            // `import std.foo.*` / `extras.foo.*` must still honor `@exports`
            // (hide raw FFI). Selective/single already validated above.
            let visible = match &imp.names {
                ImportNames::All => {
                    let exports: HashSet<String> =
                        bundled_exports(&imp.path)?.into_iter().collect();
                    import_visible_names(&dep.items, &imp.names)
                        .into_iter()
                        .filter(|n| exports.contains(n))
                        .collect()
                }
                _ => import_visible_names(&dep.items, &imp.names),
            };
            let filtered = filter_items(dep.items, &imp.names)?;
            if is_entry {
                extend_visibility(visibility, &filtered, &visible);
            } else {
                let empty = HashSet::default();
                extend_visibility(visibility, &filtered, &empty);
            }
            append_items_unique(&mut imported_items, filtered);
            continue;
        }
        let file = resolve_import_file(&importer_dir, search_roots, imp)?;
        // Canonicalize so the same file via different relative paths shares one identity
        // (cycle detection + single load).
        let file = file.canonicalize().unwrap_or(file);
        // Reject symlink (or other) escapes outside the importer dir / dep roots.
        if !path_under_search_roots(&file, &importer_dir, search_roots) {
            bail!(
                "import resolves to {} which escapes package search roots",
                file.display()
            );
        }
        let dep = load_module_file(
            &file,
            search_roots,
            overlays,
            stack,
            done,
            files,
            visibility,
            false,
        )?;
        let visible = import_visible_names(&dep.items, &imp.names);
        // Apply `as` renames before recording origins so aliases get `name_origin`.
        let filtered = filter_items(dep.items, &imp.names)?;
        // Only the entry module's imports expand the user-facing scope.
        if is_entry {
            extend_visibility(visibility, &filtered, &visible);
        } else {
            // Nested deps: record origins only (no new entry-visible names).
            let empty = HashSet::default();
            extend_visibility(visibility, &filtered, &empty);
        }
        append_items_unique(&mut imported_items, filtered);
    }

    // Std modules are inlined; drop their import nodes from the entry AST.
    m.imports.retain(|i| !is_std(&i.path) && !is_extras(&i.path));
    // Record this file's declarations (entry or dep). Entry names are visible
    // via same-file origin; deps rely on import_visible_names above.
    let local_visible: HashSet<String> = if is_entry {
        m.items
            .iter()
            .filter_map(item_name)
            .map(|s| s.to_string())
            .collect()
    } else {
        HashSet::default()
    };
    extend_visibility(visibility, &m.items, &local_visible);

    append_items_unique(&mut imported_items, std::mem::take(&mut m.items));
    check_no_duplicate_toplevel(&imported_items, files)?;
    m.items = imported_items;
    done.insert(path_key.to_path_buf(), m.clone());
    Ok(m)
}

pub fn path_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

pub(super) fn path_under_search_roots(
    path: &Path,
    importer_dir: &Path,
    search_roots: &[PathBuf],
) -> bool {
    let mut roots: Vec<PathBuf> = Vec::with_capacity(search_roots.len() + 1);
    roots.push(importer_dir.to_path_buf());
    for r in search_roots {
        if !roots.iter().any(|x| x == r) {
            roots.push(r.clone());
        }
    }
    for r in &roots {
        let root = r.canonicalize().unwrap_or_else(|_| r.clone());
        if path.starts_with(&root) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::load_program;
    use std::fs;

    #[test]
    fn import_symlink_escape_rejected() {
        let dir = std::env::temp_dir().join(format!("lumia_load_symlink_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let outside =
            std::env::temp_dir().join(format!("lumia_load_outside_{}.lm", std::process::id()));
        fs::write(&outside, "module Outside\nval leak = 1\n").unwrap();
        let entry = dir.join("main.lm");
        fs::write(&entry, "module Main\nimport evil.{leak}\nval main = leak\n").unwrap();
        #[cfg(unix)]
        {
            let evil = dir.join("evil.lm");
            let _ = fs::remove_file(&evil);
            std::os::unix::fs::symlink(&outside, &evil).unwrap();
            let err = load_program(&entry).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("escapes") || msg.contains("cannot find"),
                "expected symlink escape rejection, got {msg}"
            );
        }
        let _ = fs::remove_file(&outside);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn diamond_import_loads_shared_dep_once() {
        let dir = std::env::temp_dir().join(format!("lumia_diamond_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("c.lm"), "module C\nval c = 1\n").unwrap();
        fs::write(dir.join("a.lm"), "module A\nimport c.{c}\nval a = c\n").unwrap();
        fs::write(dir.join("b.lm"), "module B\nimport c.{c}\nval b = c\n").unwrap();
        let entry = dir.join("main.lm");
        fs::write(
            &entry,
            "module Main\nimport a.{a}\nimport b.{b}\nval main = a + b\n",
        )
        .unwrap();
        let prog = load_program(&entry).expect("diamond import should load");
        let c_count = prog
            .module
            .items
            .iter()
            .filter(|it| item_name(it) == Some("c"))
            .count();
        assert_eq!(c_count, 1, "shared dep items must be deduped");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_toplevel_name_across_modules_rejected() {
        let dir = std::env::temp_dir().join(format!("lumia_dup_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.lm"), "module A\nval conflict = 1\n").unwrap();
        fs::write(dir.join("b.lm"), "module B\nval conflict = 2\n").unwrap();
        let entry = dir.join("main.lm");
        fs::write(
            &entry,
            "module Main\nimport a.{conflict}\nimport b.{conflict}\nval main = conflict\n",
        )
        .unwrap();
        let err = load_program(&entry).unwrap_err().to_string();
        assert!(
            err.contains("duplicate top-level name") && err.contains("conflict"),
            "got {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn true_cycle_still_rejected() {
        let dir = std::env::temp_dir().join(format!("lumia_cycle_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.lm"), "module A\nimport b.{b}\nval a = b\n").unwrap();
        fs::write(dir.join("b.lm"), "module B\nimport a.{a}\nval b = a\n").unwrap();
        let entry = dir.join("main.lm");
        fs::write(&entry, "module Main\nimport a.{a}\nval main = a\n").unwrap();
        let err = load_program(&entry).unwrap_err().to_string();
        assert!(err.contains("cyclic import"), "got {err}");
        let _ = fs::remove_dir_all(&dir);
    }
}

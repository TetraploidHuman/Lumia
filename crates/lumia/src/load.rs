//! Multi-file module loading: resolve non-`std` imports relative to the entry file.
//!
//! Entire dependency modules are inlined so private callees of public APIs remain
//! linkable, but [`lumia_ty::NameVisibility`] ensures `priv` names cannot be
//! referenced from the entry module's own code.

use crate::vis::{
    apply_import_aliases, extend_visibility, import_visible_names, item_is_priv, item_name,
};
use anyhow::{bail, Context, Result};
use lumia_syntax::{
    format_diagnostic, parse_module, stamp_module, Import, ImportNames, Item, Module,
};
use lumia_ty::NameVisibility;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

fn item_file_id(it: &Item) -> u32 {
    match it {
        Item::Val(v) => v.span.file,
        Item::Type(t) => t.span.file,
        Item::Foreign(f) => f.span.file,
    }
}

/// Reject silently-overwritten top-level names after import inlining.
fn check_no_duplicate_toplevel(items: &[Item], files: &[SourceFile]) -> Result<()> {
    let mut seen: HashMap<&str, u32> = HashMap::new();
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
fn append_items_unique(dst: &mut Vec<Item>, src: Vec<Item>) {
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
    let mut stack = HashSet::new();
    let mut done = HashMap::new();
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
fn collect_std_aliases(imp: &Import, out: &mut HashMap<String, String>) -> Result<()> {
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

fn filter_items(items: Vec<Item>, names: &ImportNames) -> Result<Vec<Item>> {
    let (privs, pubs): (Vec<_>, Vec<_>) = items.into_iter().partition(item_is_priv);
    match names {
        ImportNames::All => {
            let mut out = pubs;
            out.extend(privs);
            Ok(out)
        }
        ImportNames::Single(n) => {
            if privs
                .iter()
                .any(|it| item_name(it) == Some(n.name.as_str()))
            {
                bail!("cannot import private `{}`", n.name);
            }
            if !pubs
                .iter()
                .any(|it| item_name(it) == Some(n.name.as_str()))
            {
                bail!("module has no public `{}`", n.name);
            }
            // Keep whole module for private callees of public APIs; visibility
            // is enforced separately via NameVisibility.
            let mut out = pubs;
            out.extend(privs);
            Ok(apply_import_aliases(out, names))
        }
        ImportNames::Selective(ns) => {
            for n in ns {
                if privs
                    .iter()
                    .any(|it| item_name(it) == Some(n.name.as_str()))
                {
                    bail!("cannot import private `{}`", n.name);
                }
                if !pubs
                    .iter()
                    .any(|it| item_name(it) == Some(n.name.as_str()))
                {
                    bail!("module has no public `{}`", n.name);
                }
            }
            let mut out = pubs;
            out.extend(privs);
            Ok(apply_import_aliases(out, names))
        }
    }
}

fn load_module_file(
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

fn load_module_file_uncached(
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

    if is_entry {
        visibility.entry_file = file_id;
    }

    let mut imported_items = Vec::new();
    for imp in &m.imports {
        if is_std(&imp.path) {
            validate_std_import(imp)?;
            if is_entry {
                collect_std_aliases(imp, &mut visibility.builtin_aliases)?;
            }
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
            let empty = HashSet::new();
            extend_visibility(visibility, &filtered, &empty);
        }
        append_items_unique(&mut imported_items, filtered);
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

    append_items_unique(&mut imported_items, std::mem::take(&mut m.items));
    check_no_duplicate_toplevel(&imported_items, files)?;
    m.items = imported_items;
    done.insert(path_key.to_path_buf(), m.clone());
    Ok(m)
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn path_under_search_roots(path: &Path, importer_dir: &Path, search_roots: &[PathBuf]) -> bool {
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
    use std::fs;

    #[test]
    fn import_symlink_escape_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "lumia_load_symlink_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let outside = std::env::temp_dir().join(format!(
            "lumia_load_outside_{}.lumia",
            std::process::id()
        ));
        fs::write(
            &outside,
            "module Outside\nval leak = 1\n",
        )
        .unwrap();
        let entry = dir.join("main.lumia");
        fs::write(
            &entry,
            "module Main\nimport evil.{leak}\nval main = leak\n",
        )
        .unwrap();
        let evil = dir.join("evil.lumia");
        #[cfg(unix)]
        {
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
        fs::write(&dir.join("c.lumia"), "module C\nval c = 1\n").unwrap();
        fs::write(
            &dir.join("a.lumia"),
            "module A\nimport c.{c}\nval a = c\n",
        )
        .unwrap();
        fs::write(
            &dir.join("b.lumia"),
            "module B\nimport c.{c}\nval b = c\n",
        )
        .unwrap();
        let entry = dir.join("main.lumia");
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
        fs::write(&dir.join("a.lumia"), "module A\nval conflict = 1\n").unwrap();
        fs::write(&dir.join("b.lumia"), "module B\nval conflict = 2\n").unwrap();
        let entry = dir.join("main.lumia");
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
        fs::write(
            &dir.join("a.lumia"),
            "module A\nimport b.{b}\nval a = b\n",
        )
        .unwrap();
        fs::write(
            &dir.join("b.lumia"),
            "module B\nimport a.{a}\nval b = a\n",
        )
        .unwrap();
        let entry = dir.join("main.lumia");
        fs::write(&entry, "module Main\nimport a.{a}\nval main = a\n").unwrap();
        let err = load_program(&entry).unwrap_err().to_string();
        assert!(err.contains("cyclic import"), "got {err}");
        let _ = fs::remove_dir_all(&dir);
    }
}

/// Rewrite entry-module idents that are std `as` aliases (e.g. `log` → `println`)
/// so HIR builtin lowering still matches canonical names.
fn rewrite_builtin_alias_idents(m: &mut Module, aliases: &HashMap<String, String>, entry_file: u32) {
    if aliases.is_empty() {
        return;
    }
    for it in &mut m.items {
        // Only rewrite code that originated in the entry file.
        let file = item_file_id(it);
        if file != entry_file {
            continue;
        }
        match it {
            Item::Val(v) => rewrite_expr_aliases(&mut v.body, aliases),
            Item::Type(_) | Item::Foreign(_) => {}
        }
    }
}

fn rewrite_expr_aliases(e: &mut lumia_syntax::Expr, aliases: &HashMap<String, String>) {
    use lumia_syntax::Expr::*;
    match e {
        Ident(name, _) => {
            if let Some(canon) = aliases.get(name) {
                *name = canon.clone();
            }
        }
        Interp { parts, .. } => {
            for p in parts {
                if let lumia_syntax::InterpPart::Expr(ex) = p {
                    rewrite_expr_aliases(ex, aliases);
                }
            }
        }
        Block { stmts, tail, .. } => {
            for s in stmts {
                rewrite_stmt_aliases(s, aliases);
            }
            if let Some(t) = tail {
                rewrite_expr_aliases(t, aliases);
            }
        }
        Lambda { body, .. } => rewrite_expr_aliases(body, aliases),
        Call { callee, args, .. } => {
            rewrite_expr_aliases(callee, aliases);
            for a in args {
                rewrite_expr_aliases(a, aliases);
            }
        }
        Binary { left, right, .. } | Pipeline { left, right, .. } => {
            rewrite_expr_aliases(left, aliases);
            rewrite_expr_aliases(right, aliases);
        }
        Unary { expr, .. } => rewrite_expr_aliases(expr, aliases),
        If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_expr_aliases(cond, aliases);
            rewrite_expr_aliases(then_branch, aliases);
            if let Some(e) = else_branch {
                rewrite_expr_aliases(e, aliases);
            }
        }
        Match {
            scrutinee, arms, ..
        } => {
            rewrite_expr_aliases(scrutinee, aliases);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    rewrite_expr_aliases(g, aliases);
                }
                rewrite_expr_aliases(&mut a.body, aliases);
            }
        }
        MatchCond { arms, .. } => {
            for a in arms {
                if let Some(c) = &mut a.cond {
                    rewrite_expr_aliases(c, aliases);
                }
                rewrite_expr_aliases(&mut a.body, aliases);
            }
        }
        Field { base, .. } => rewrite_expr_aliases(base, aliases),
        ListLit { elems, .. } | TupleLit { elems, .. } => {
            for el in elems {
                rewrite_expr_aliases(el, aliases);
            }
        }
        StructLit { fields, .. } => {
            for (_, ex) in fields {
                rewrite_expr_aliases(ex, aliases);
            }
        }
        With { base, fields, .. } => {
            rewrite_expr_aliases(base, aliases);
            for (_, ex) in fields {
                rewrite_expr_aliases(ex, aliases);
            }
        }
        Int(..) | Float(..) | Bool(..) | String(..) | Char(..) => {}
    }
}

fn rewrite_stmt_aliases(s: &mut lumia_syntax::Stmt, aliases: &HashMap<String, String>) {
    use lumia_syntax::Stmt::*;
    match s {
        Val { expr, .. } | Var { expr, .. } | Assign { expr, .. } => {
            rewrite_expr_aliases(expr, aliases)
        }
        Expr(expr) => rewrite_expr_aliases(expr, aliases),
        ForIn { iter, body, .. } => {
            rewrite_expr_aliases(iter, aliases);
            rewrite_expr_aliases(body, aliases);
        }
        ForCond { cond, body, .. } => {
            rewrite_expr_aliases(cond, aliases);
            rewrite_expr_aliases(body, aliases);
        }
        Break(_) | Continue(_) => {}
    }
}

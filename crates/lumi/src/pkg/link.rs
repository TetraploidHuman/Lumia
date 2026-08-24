//! Linker flag validation and collection from package manifests.

use super::manifest::{load_manifest, resolve_dep_path, Manifest};
use anyhow::{bail, Result};
use rustc_hash::FxHashSet as HashSet;
use std::path::{Path, PathBuf};

/// Collect `package.link` flags from the root manifest (+ transitive, unique).
pub fn collect_link_args(manifest_path: &Path, m: &Manifest) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut seen_pkg = HashSet::default();
    let root = manifest_path.parent().unwrap_or(Path::new("."));
    collect_link_args_rec(root, m, &mut out, &mut seen_pkg, 0)?;
    Ok(out)
}

fn collect_link_args_rec(
    root: &Path,
    m: &Manifest,
    out: &mut Vec<String>,
    seen_pkg: &mut HashSet<String>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        bail!("dependency nesting too deep while collecting link flags");
    }
    if !seen_pkg.insert(m.package.name.clone()) {
        return Ok(());
    }
    for a in &m.package.link {
        let resolved = resolve_link_arg(root, a)?;
        if !out.iter().any(|x| x == &resolved) {
            out.push(resolved);
        }
    }
    for (name, spec) in &m.dependencies {
        let path = resolve_dep_path(root, name, spec)?;
        let dep_manifest = path.join("Lumi.toml");
        if dep_manifest.is_file() {
            let dep_m = load_manifest(&dep_manifest)?;
            let dep_root = dep_manifest.parent().unwrap_or(Path::new("."));
            collect_link_args_rec(dep_root, &dep_m, out, seen_pkg, depth + 1)?;
        }
    }
    Ok(())
}

/// Where a link flag comes from — package manifests are confined to the
/// package root; CLI `--link` may use absolute paths (explicit user intent).
#[derive(Debug, Clone, Copy)]
enum LinkArgKind {
    Package,
    Cli,
}

/// Validate a `package.link` entry and resolve relative `-L` / archive paths
/// against `package_root` (manifest parent), then canonicalize under that root.
fn resolve_link_arg(package_root: &Path, arg: &str) -> Result<String> {
    resolve_link_arg_inner(package_root, arg, LinkArgKind::Package)
}

/// Validate a CLI `--link` flag. Same allowlist as `package.link`, but paths
/// may be absolute and are resolved against `cwd` (not confined to a package).
pub fn validate_cli_link_arg(cwd: &Path, arg: &str) -> Result<String> {
    resolve_link_arg_inner(cwd, arg, LinkArgKind::Cli)
}

fn resolve_link_arg_inner(base: &Path, arg: &str, kind: LinkArgKind) -> Result<String> {
    let label = match kind {
        LinkArgKind::Package => "package.link",
        LinkArgKind::Cli => "--link",
    };
    if arg.is_empty() {
        bail!("empty {label} entry");
    }
    if arg.starts_with('@') {
        bail!("{label} response files (@…) are not allowed: `{arg}`");
    }
    if arg == "-Wl" || arg.starts_with("-Wl,") || arg == "-Xlinker" || arg.starts_with("-Xlinker=")
    {
        bail!("{label} linker-passthrough flags are not allowed: `{arg}`");
    }
    if let Some(path) = arg.strip_prefix("-L") {
        if path.is_empty() {
            bail!("{label} `-L` requires a path");
        }
        let abs = resolve_link_path(base, path, arg, kind)?;
        return Ok(format!("-L{}", abs.display()));
    }
    if arg.starts_with("-l") || arg.starts_with("-framework") {
        // Library names only — no path separators.
        let rest = arg
            .trim_start_matches("-framework")
            .trim_start_matches("-l");
        if rest.contains('/') || rest.contains('\\') || rest.contains("..") {
            bail!("{label} library name must not contain path segments: `{arg}`");
        }
        return Ok(arg.to_string());
    }
    // Allow plain archive/object paths.
    if arg.ends_with(".a") || arg.ends_with(".lib") || arg.ends_with(".o") || arg.ends_with(".obj")
    {
        let abs = resolve_link_path(base, arg, arg, kind)?;
        return Ok(abs.display().to_string());
    }
    bail!("{label} entry `{arg}` not allowed (use -lNAME, -Lpath, -framework, or a .a/.o path)");
}

fn validate_link_path(path: &str, original: &str, kind: LinkArgKind) -> Result<()> {
    let label = match kind {
        LinkArgKind::Package => "package.link",
        LinkArgKind::Cli => "--link",
    };
    if Path::new(path).is_absolute() {
        if matches!(kind, LinkArgKind::Package) {
            bail!("{label} path must be relative (got absolute in `{original}`)");
        }
        return Ok(());
    }
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        bail!("{label} path must not contain `..`: `{original}`");
    }
    Ok(())
}

fn resolve_link_path(
    base: &Path,
    path: &str,
    original: &str,
    kind: LinkArgKind,
) -> Result<PathBuf> {
    validate_link_path(path, original, kind)?;
    if Path::new(path).is_absolute() {
        // CLI only (validated above).
        return Ok(PathBuf::from(path));
    }
    let joined = base.join(path);
    let root_canon = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
    // Walk existing prefixes so a symlink planted on a not-yet-created leaf
    // (or intermediate) cannot escape the package root when it later appears.
    if matches!(kind, LinkArgKind::Package) {
        let mut cur = root_canon.clone();
        for c in Path::new(path).components() {
            use std::path::Component;
            match c {
                Component::Normal(s) => cur.push(s),
                Component::CurDir => {}
                _ => bail!("package.link path `{original}` is not relative-normal"),
            }
            if cur.exists() {
                let canon = cur.canonicalize().unwrap_or_else(|_| cur.clone());
                if !canon.starts_with(&root_canon) {
                    bail!(
                        "package.link path `{}` escapes package root {}",
                        original,
                        root_canon.display()
                    );
                }
                cur = canon;
            }
        }
    }
    let resolved = if joined.exists() {
        joined.canonicalize().unwrap_or_else(|_| joined.clone())
    } else {
        root_canon.join(path)
    };
    Ok(resolved)
}

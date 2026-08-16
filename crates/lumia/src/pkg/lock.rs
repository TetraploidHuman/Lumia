//! Lumia lockfile (`Lumia.lock`).

use super::manifest::{load_manifest, resolve_dep_path, DepSpec, DepTable, Manifest};
use anyhow::{bail, Context, Result};
use rustc_hash::FxHashSet as HashSet;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Lockfile {
    pub package: Vec<LockPackage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LockPackage {
    pub name: String,
    pub version: String,
    pub path: String,
}

pub fn write_lockfile(path: &Path, lock: &Lockfile) -> Result<()> {
    let s = toml::to_string_pretty(lock).context("serialize Lumia.lock")?;
    fs::write(path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn load_lockfile(path: &Path) -> Result<Lockfile> {
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&src).with_context(|| format!("parse {}", path.display()))
}

/// Build a lockfile from the manifest (path pins; versions from dep `Lumia.toml` when present).
pub fn lock_from_manifest(manifest_path: &Path, m: &Manifest) -> Result<Lockfile> {
    let root = manifest_path.parent().unwrap_or(Path::new("."));
    let mut packages = Vec::new();
    packages.push(LockPackage {
        name: m.package.name.clone(),
        version: m.package.version.clone(),
        path: ".".into(),
    });
    let mut seen = HashSet::default();
    seen.insert(m.package.name.clone());
    lock_deps_recursive(root, m, &mut packages, &mut seen, root, 0)?;
    packages.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Lockfile { package: packages })
}

fn lock_deps_recursive(
    root: &Path,
    m: &Manifest,
    packages: &mut Vec<LockPackage>,
    seen: &mut HashSet<String>,
    lock_root: &Path,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        bail!("dependency nesting too deep while locking");
    }
    for (name, spec) in &m.dependencies {
        if !seen.insert(name.clone()) {
            continue;
        }
        let abs = resolve_dep_path(root, name, spec)?;
        let rel = pathdiff_rel(lock_root, &abs).unwrap_or_else(|| abs.display().to_string());
        let version = match spec {
            DepSpec::Version(v) => v.clone(),
            DepSpec::Table(DepTable { version, .. }) => version
                .clone()
                .or_else(|| read_package_version(&abs))
                .unwrap_or_else(|| "0.0.0".into()),
        };
        packages.push(LockPackage {
            name: name.clone(),
            version,
            path: rel,
        });
        let dep_manifest = abs.join("Lumia.toml");
        if dep_manifest.is_file() {
            let dep_m = load_manifest(&dep_manifest)?;
            let dep_root = dep_manifest.parent().unwrap_or(Path::new("."));
            lock_deps_recursive(dep_root, &dep_m, packages, seen, lock_root, depth + 1)?;
        }
    }
    Ok(())
}

fn pathdiff_rel(from: &Path, to: &Path) -> Option<String> {
    let to = to.canonicalize().ok()?;
    let from = from.canonicalize().ok()?;
    let to_s = to.to_string_lossy();
    let from_s = from.to_string_lossy();
    if let Some(rest) = to_s.strip_prefix(from_s.as_ref()) {
        let rest = rest.trim_start_matches(['/', '\\']);
        if rest.is_empty() {
            return Some(".".into());
        }
        return Some(rest.replace('\\', "/"));
    }
    Some(to.display().to_string())
}

fn read_package_version(dep_root: &Path) -> Option<String> {
    let cand = if dep_root.is_file() {
        return None;
    } else {
        dep_root.join("Lumia.toml")
    };
    load_manifest(&cand).ok().map(|m| m.package.version)
}

/// Verify `Lumia.lock` against the manifest: every expected package is present
/// with matching path/version, and the lock has no stale extra entries.
pub fn verify_lockfile(manifest_path: &Path, m: &Manifest, lock: &Lockfile) -> Result<()> {
    let expected = lock_from_manifest(manifest_path, m)?;
    let expected_names: HashSet<String> = expected.package.iter().map(|p| p.name.clone()).collect();
    for got in &lock.package {
        if !expected_names.contains(&got.name) {
            bail!(
                "Lumia.lock has unexpected package `{}` (run `lumia pkg lock`)",
                got.name
            );
        }
    }
    for exp in &expected.package {
        let Some(got) = lock.package.iter().find(|p| p.name == exp.name) else {
            bail!(
                "Lumia.lock missing package `{}` (run `lumia pkg lock`)",
                exp.name
            );
        };
        if got.path != exp.path {
            bail!(
                "Lumia.lock path for `{}` is `{}`, expected `{}` (run `lumia pkg lock`)",
                exp.name,
                got.path,
                exp.path
            );
        }
        if got.version != exp.version {
            bail!(
                "Lumia.lock version for `{}` is `{}`, expected `{}` (run `lumia pkg lock`)",
                exp.name,
                got.version,
                exp.version
            );
        }
        if exp.path == "." {
            continue;
        }
        let abs = manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&got.path);
        if !abs.exists() {
            bail!(
                "locked dependency `{}` path {} does not exist",
                exp.name,
                abs.display()
            );
        }
    }
    Ok(())
}

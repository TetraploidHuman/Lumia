//! Lumia package manifest (`Lumia.toml`) + lockfile (`Lumia.lock`).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub package: PackageMeta,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DepSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PackageMeta {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Extra linker flags, e.g. `["-lm", "-L/opt/lib"]`.
    #[serde(default)]
    pub link: Vec<String>,
}

fn default_version() -> String {
    "0.1.0".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DepSpec {
    /// `foo = "0.1"` — version req (path resolved under `./deps/foo` or vendor)
    Version(String),
    Table {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        version: Option<String>,
    },
}

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

/// Walk parents from `start` looking for `Lumia.toml`.
pub fn find_manifest(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };
    loop {
        let cand = dir.join("Lumia.toml");
        if cand.is_file() {
            return Some(cand);
        }
        if !dir.pop() {
            return None;
        }
    }
}

pub fn load_manifest(path: &Path) -> Result<Manifest> {
    let src = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&src).with_context(|| format!("parse {}", path.display()))
}

pub fn write_manifest(path: &Path, m: &Manifest) -> Result<()> {
    let s = toml::to_string_pretty(m).context("serialize Lumia.toml")?;
    fs::write(path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
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

fn resolve_dep_path(root: &Path, name: &str, spec: &DepSpec) -> Result<PathBuf> {
    let path = match spec {
        DepSpec::Table {
            path: Some(p), ..
        } => root.join(p),
        DepSpec::Version(_) | DepSpec::Table { path: None, .. } => {
            let vendor = root.join("deps").join(name);
            if vendor.is_dir() {
                vendor
            } else {
                root.join("vendor").join(name)
            }
        }
    };
    if !path.exists() {
        bail!(
            "dependency `{name}` path {} does not exist (run `lumia pkg lock` after vendoring)",
            path.display()
        );
    }
    Ok(path.canonicalize().unwrap_or(path))
}

/// Resolve dependency search roots (direct + transitive path deps).
pub fn dependency_roots(manifest_path: &Path, m: &Manifest) -> Result<Vec<PathBuf>> {
    let root = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut roots = vec![root.clone()];
    let mut seen = HashSet::new();
    seen.insert(root.canonicalize().unwrap_or(root.clone()));
    collect_dep_roots(&root, m, &mut roots, &mut seen, 0)?;
    Ok(roots)
}

fn collect_dep_roots(
    root: &Path,
    m: &Manifest,
    roots: &mut Vec<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        bail!("dependency nesting too deep (cycle?)");
    }
    for (name, spec) in &m.dependencies {
        let path = resolve_dep_path(root, name, spec)?;
        let key = path.canonicalize().unwrap_or(path.clone());
        if !seen.insert(key) {
            continue;
        }
        roots.push(path.clone());
        // Transitive: if dep has its own Lumia.toml, pull its deps too.
        let dep_manifest = if path.is_dir() {
            path.join("Lumia.toml")
        } else {
            PathBuf::new()
        };
        if dep_manifest.is_file() {
            let dep_m = load_manifest(&dep_manifest)?;
            let dep_root = dep_manifest.parent().unwrap_or(Path::new("."));
            collect_dep_roots(dep_root, &dep_m, roots, seen, depth + 1)?;
        }
    }
    Ok(())
}

/// Collect `package.link` flags from the root manifest (+ transitive, unique).
pub fn collect_link_args(manifest_path: &Path, m: &Manifest) -> Result<Vec<String>> {
    let mut out = m.package.link.clone();
    let root = manifest_path.parent().unwrap_or(Path::new("."));
    let mut seen_pkg = HashSet::new();
    seen_pkg.insert(m.package.name.clone());
    for (name, spec) in &m.dependencies {
        let path = resolve_dep_path(root, name, spec)?;
        let dep_manifest = path.join("Lumia.toml");
        if dep_manifest.is_file() {
            let dep_m = load_manifest(&dep_manifest)?;
            if seen_pkg.insert(dep_m.package.name.clone()) {
                for a in &dep_m.package.link {
                    if !out.iter().any(|x| x == a) {
                        out.push(a.clone());
                    }
                }
            }
        }
    }
    Ok(out)
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
    let mut seen = HashSet::new();
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
            DepSpec::Table { version, .. } => version
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
    load_manifest(&cand)
        .ok()
        .map(|m| m.package.version)
}

/// Verify `Lumia.lock` against the manifest: every dep is present with matching path/version.
pub fn verify_lockfile(manifest_path: &Path, m: &Manifest, lock: &Lockfile) -> Result<()> {
    let expected = lock_from_manifest(manifest_path, m)?;
    for exp in &expected.package {
        if exp.path == "." {
            continue;
        }
        let Some(got) = lock.package.iter().find(|p| p.name == exp.name) else {
            bail!(
                "Lumia.lock missing dependency `{}` (run `lumia pkg lock`)",
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

pub fn init_manifest(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join("Lumia.toml");
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    let body = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
link = []

[dependencies]
"#
    );
    fs::write(&path, body)?;
    Ok(path)
}

/// Add a path dependency and rewrite the manifest.
pub fn add_path_dep(manifest_path: &Path, name: &str, dep_path: &str) -> Result<()> {
    let mut m = load_manifest(manifest_path)?;
    if m.dependencies.contains_key(name) {
        bail!("dependency `{name}` already exists");
    }
    m.dependencies.insert(
        name.to_string(),
        DepSpec::Table {
            path: Some(dep_path.to_string()),
            version: None,
        },
    );
    write_manifest(manifest_path, &m)?;
    Ok(())
}

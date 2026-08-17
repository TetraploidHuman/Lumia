//! Lumia lockfile (`Lumia.lock`).

use super::manifest::{load_manifest, resolve_dep_path, DepSpec, DepTable, Manifest};
use anyhow::{bail, Context, Result};
use rustc_hash::FxHashSet as HashSet;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Lockfile {
    pub package: Vec<LockPackage>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockPackage {
    pub name: String,
    pub version: String,
    pub path: String,
    /// Stable FNV-1a fingerprint of dep `Lumia.toml` + sorted `*.lm` sources.
    /// Empty for the root package (`path == "."`).
    #[serde(default)]
    pub content: String,
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
        content: String::new(),
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
        let pkg_ver = read_package_version(&abs);
        let version = match spec {
            DepSpec::Version(constraint) => {
                let Some(pkg_ver) = pkg_ver else {
                    bail!(
                        "dependency `{name}` = \"{constraint}\" has no `package.version` \
                         (add Lumia.toml under its path)"
                    );
                };
                if pkg_ver != *constraint {
                    bail!(
                        "dependency `{name}` constraint `{constraint}` does not match \
                         package.version `{pkg_ver}` in its Lumia.toml"
                    );
                }
                pkg_ver
            }
            DepSpec::Table(DepTable { version, .. }) => match (version.as_ref(), pkg_ver) {
                (Some(pin), Some(ref pv)) if pin != pv => bail!(
                    "dependency `{name}` version `{pin}` does not match \
                     package.version `{pv}` in its Lumia.toml"
                ),
                (Some(pin), _) => pin.clone(),
                (None, Some(pv)) => pv,
                (None, None) => bail!(
                    "dependency `{name}` has no version (set \
                     `dependencies.{name}.version` or `package.version` in its Lumia.toml)"
                ),
            },
        };
        let content = package_content_fingerprint(&abs)?;
        packages.push(LockPackage {
            name: name.clone(),
            version,
            path: rel,
            content,
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

/// FNV-1a 64-bit — stable across Rust versions (unlike `DefaultHasher`).
fn fnv1a64(data: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Fingerprint dep package sources so vendor edits without a version bump fail verify.
fn package_content_fingerprint(dep_root: &Path) -> Result<String> {
    let mut acc = Vec::new();
    let toml = dep_root.join("Lumia.toml");
    if toml.is_file() {
        acc.extend(b"toml\0");
        acc.extend(fs::read(&toml).with_context(|| format!("read {}", toml.display()))?);
    }
    let mut lms = Vec::new();
    collect_lm_rel_paths(dep_root, dep_root, &mut lms)?;
    lms.sort();
    for rel in lms {
        acc.extend(b"lm\0");
        acc.extend(rel.as_bytes());
        acc.push(0);
        let abs = dep_root.join(&rel);
        acc.extend(fs::read(&abs).with_context(|| format!("read {}", abs.display()))?);
    }
    Ok(format!("{:016x}", fnv1a64(&acc)))
}

fn collect_lm_rel_paths(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))?;
    for ent in entries {
        let ent = ent.with_context(|| format!("read dir entry under {}", dir.display()))?;
        let path = ent.path();
        let name = ent.file_name();
        let name = name.to_string_lossy();
        // Skip nested package trees / VCS / build outputs.
        if name == "deps" || name == "vendor" || name == "target" || name == ".git" {
            continue;
        }
        if path.is_dir() {
            collect_lm_rel_paths(root, &path, out)?;
        } else if name.ends_with(".lm") {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

/// Search roots from a verified lockfile (lock-driven paths, not manifest re-resolve).
pub fn dependency_roots_from_lock(manifest_path: &Path, lock: &Lockfile) -> Result<Vec<PathBuf>> {
    let root = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut roots = vec![root.clone()];
    let mut seen = HashSet::default();
    seen.insert(root.canonicalize().unwrap_or(root.clone()));
    for pkg in &lock.package {
        if pkg.path == "." {
            continue;
        }
        let abs = root.join(&pkg.path);
        let key = abs.canonicalize().unwrap_or(abs.clone());
        if !seen.insert(key) {
            continue;
        }
        if !abs.exists() {
            bail!(
                "locked dependency `{}` path {} does not exist",
                pkg.name,
                abs.display()
            );
        }
        roots.push(abs);
    }
    Ok(roots)
}

/// Verify `Lumia.lock` against the manifest: every expected package is present
/// with matching path/version/content, and the lock has no stale extra entries.
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
        if got.content != exp.content {
            bail!(
                "Lumia.lock content fingerprint for `{}` is `{}`, expected `{}` \
                 (dependency sources changed; run `lumia pkg lock`)",
                exp.name,
                got.content,
                exp.content
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

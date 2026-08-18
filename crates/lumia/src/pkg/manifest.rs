//! Lumia package manifest (`Lumia.toml`).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub package: PackageMeta,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DepSpec>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageMeta {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Extra linker flags, e.g. `["-lm", "-L/opt/lib"]`.
    #[serde(default)]
    pub link: Vec<String>,
    /// Trust `foreign "C" pure` annotations (FFI purity is not verified).
    /// Honor system: same trust surface as `--trust-foreign-pure` / `--link` for
    /// untrusted trees — prefer leaving this false and passing the CLI flag when needed.
    #[serde(default)]
    pub trust_foreign_pure: bool,
}

pub(super) fn default_version() -> String {
    "0.1.0".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DepTable {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DepSpec {
    /// `foo = "0.1"` — **not** a semver constraint. Resolved only as a directory
    /// under `./deps/<name>` or `./vendor/<name>` (no registry / version solve).
    Version(String),
    Table(DepTable),
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

pub(super) fn resolve_dep_path(root: &Path, name: &str, spec: &DepSpec) -> Result<PathBuf> {
    let path = match spec {
        DepSpec::Table(DepTable { path: Some(p), .. }) => {
            if Path::new(p).is_absolute() {
                bail!(
                    "dependency `{name}` path must be relative to the package root (got absolute `{p}`)"
                );
            }
            if p.split(['/', '\\']).any(|seg| seg == "..") {
                bail!("dependency `{name}` path must not contain `..` segments (got `{p}`)");
            }
            root.join(p)
        }
        DepSpec::Version(ver) => {
            let deps = root.join("deps").join(name);
            let vendor = root.join("vendor").join(name);
            if deps.is_dir() {
                deps
            } else if vendor.is_dir() {
                vendor
            } else {
                bail!(
                    "dependency `{name}` = \"{ver}\" is not a semver solve — looked for \
                     `./deps/{name}` and `./vendor/{name}` (no registry/git); vendor the \
                     package or use `{{ path = \"...\" }}`"
                );
            }
        }
        DepSpec::Table(DepTable {
            path: None,
            version,
            ..
        }) => {
            let deps = root.join("deps").join(name);
            let vendor = root.join("vendor").join(name);
            if deps.is_dir() {
                deps
            } else if vendor.is_dir() {
                vendor
            } else {
                let hint = version
                    .as_deref()
                    .map(|v| format!(" (version field `{v}` is ignored for resolution)"))
                    .unwrap_or_default();
                bail!(
                    "dependency `{name}`{hint}: no `./deps/{name}` or `./vendor/{name}` \
                     directory (path-only package layout; no registry)"
                );
            }
        }
    };
    if !path.exists() {
        bail!(
            "dependency `{name}` path {} does not exist (run `lumia pkg lock` after vendoring)",
            path.display()
        );
    }
    let canon = path.canonicalize().unwrap_or(path);
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !canon.starts_with(&root_canon) {
        bail!(
            "dependency `{name}` path {} escapes package root {}",
            canon.display(),
            root_canon.display()
        );
    }
    Ok(canon)
}

/// Resolve dependency search roots (direct + transitive path deps).
pub fn dependency_roots(manifest_path: &Path, m: &Manifest) -> Result<Vec<PathBuf>> {
    let root = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut roots = vec![root.clone()];
    let mut seen = HashSet::default();
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

pub fn init_manifest(dir: &Path, name: &str) -> Result<PathBuf> {
    let path = dir.join("Lumia.toml");
    if path.exists() {
        bail!("{} already exists", path.display());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || name.is_empty()
    {
        bail!("package name must be non-empty [A-Za-z0-9_-]+ (got `{name}`)");
    }
    let m = Manifest {
        package: PackageMeta {
            name: name.to_string(),
            version: default_version(),
            link: vec![],
            trust_foreign_pure: false,
        },
        dependencies: BTreeMap::new(),
    };
    write_manifest(&path, &m)?;
    Ok(path)
}

/// Add a path dependency and rewrite the manifest.
pub fn add_path_dep(manifest_path: &Path, name: &str, dep_path: &str) -> Result<()> {
    let mut m = load_manifest(manifest_path)?;
    if m.dependencies.contains_key(name) {
        bail!("dependency `{name}` already exists");
    }
    let root = manifest_path.parent().unwrap_or(Path::new("."));
    // Validate before writing so a rejected path cannot corrupt Lumia.toml.
    let spec = DepSpec::Table(DepTable {
        path: Some(dep_path.to_string()),
        version: None,
    });
    let _ = resolve_dep_path(root, name, &spec)?;
    m.dependencies.insert(name.to_string(), spec);
    write_manifest(manifest_path, &m)?;
    Ok(())
}

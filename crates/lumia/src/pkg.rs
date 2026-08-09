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
    /// Trust `foreign "C" pure` annotations (FFI purity is not verified).
    #[serde(default)]
    pub trust_foreign_pure: bool,
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
        DepSpec::Table { path: Some(p), .. } => {
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
        let dep_manifest = path.join("Lumia.toml");
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
    load_manifest(&cand).ok().map(|m| m.package.version)
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
    let spec = DepSpec::Table {
        path: Some(dep_path.to_string()),
        version: None,
    };
    let _ = resolve_dep_path(root, name, &spec)?;
    m.dependencies.insert(name.to_string(), spec);
    write_manifest(manifest_path, &m)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn add_path_dep_rejects_dotdot_without_writing() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_add_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "t"
version = "0.1.0"
"#,
        )
        .unwrap();
        let before = fs::read_to_string(&manifest).unwrap();
        let err = add_path_dep(&manifest, "evil", "../outside").unwrap_err();
        assert!(
            err.to_string().contains(".."),
            "expected .. rejection, got {err}"
        );
        let after = fs::read_to_string(&manifest).unwrap();
        assert_eq!(before, after, "failed add must not rewrite manifest");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn link_args_resolve_relative_to_package_root() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_link_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("lib")).unwrap();
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "t"
version = "0.1.0"
link = ["-Llib", "-lm"]
"#,
        )
        .unwrap();
        let m = load_manifest(&manifest).unwrap();
        let args = collect_link_args(&manifest, &m).unwrap();
        assert!(args.iter().any(|a| a == "-lm"));
        let lflag = args.iter().find(|a| a.starts_with("-L")).unwrap();
        let path = Path::new(lflag.strip_prefix("-L").unwrap());
        assert!(path.is_absolute(), "expected absolute -L, got {lflag}");
        assert!(
            path.ends_with("lib") || path.file_name().is_some_and(|n| n == "lib"),
            "expected …/lib, got {lflag}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_link_rejects_response_file_and_wl() {
        let cwd = Path::new(".");
        let err = validate_cli_link_arg(cwd, "@evil.rsp").unwrap_err();
        assert!(err.to_string().contains("@"), "{err}");
        let err = validate_cli_link_arg(cwd, "-Wl,-foo").unwrap_err();
        assert!(
            err.to_string().contains("-Wl") || err.to_string().contains("passthrough"),
            "{err}"
        );
        let err = validate_cli_link_arg(cwd, "-Xlinker").unwrap_err();
        assert!(
            err.to_string().contains("passthrough") || err.to_string().contains("-Xlinker"),
            "{err}"
        );
    }

    #[test]
    fn cli_link_allows_absolute_l_and_libname() {
        let cwd = Path::new(".");
        let a = validate_cli_link_arg(cwd, "-lm").unwrap();
        assert_eq!(a, "-lm");
        // Absolute `-L` is re-canonicalized (Windows may yield `\\?\…`).
        let a = validate_cli_link_arg(cwd, "-L/usr/lib").unwrap();
        assert!(a.starts_with("-L"), "got {a}");
        let path = Path::new(a.strip_prefix("-L").unwrap());
        assert!(path.is_absolute(), "expected absolute -L, got {a}");
    }
}

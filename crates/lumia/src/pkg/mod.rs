//! Lumia package manifest (`Lumia.toml`) + lockfile (`Lumia.lock`).

mod link;
mod lock;
mod manifest;

pub use link::{collect_link_args, validate_cli_link_arg};
pub use lock::{
    dependency_roots_from_lock, diff_lockfiles, load_lockfile, lock_from_manifest, lockfile_path,
    outdated_lock, verify_lockfile, write_lock_from_manifest, write_lockfile, LockDiff,
    LockPackage, LockPkgChange, LockWrite, Lockfile,
};
pub use manifest::{
    add_path_dep, dependency_roots, find_manifest, init_manifest, load_manifest, remove_dep,
    write_manifest, DepSpec, DepTable, Manifest, PackageMeta,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

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

    #[test]
    fn manifest_rejects_unknown_package_fields() {
        let err = toml::from_str::<Manifest>(
            r#"[package]
name = "t"
version = "0.1.0"
trust_foreign_puer = true
"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("trust_foreign_puer") || msg.contains("unknown"),
            "expected unknown-field error, got {msg}"
        );
    }

    #[test]
    fn verify_lockfile_checks_root_version_and_rejects_extras() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_verify_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"
"#,
        )
        .unwrap();
        let m = load_manifest(&manifest).unwrap();
        let lock = Lockfile {
            package: vec![
                LockPackage {
                    name: "root".into(),
                    version: "0.9.0".into(),
                    path: ".".into(),
                    content: String::new(),
                },
                LockPackage {
                    name: "stale".into(),
                    version: "0.1.0".into(),
                    path: "deps/stale".into(),
                    content: String::new(),
                },
            ],
        };
        let err = verify_lockfile(&manifest, &m, &lock)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("stale") || err.contains("unexpected"),
            "expected extra package rejection, got {err}"
        );
        let lock2 = Lockfile {
            package: vec![LockPackage {
                name: "root".into(),
                version: "0.9.0".into(),
                path: ".".into(),
                content: String::new(),
            }],
        };
        let err2 = verify_lockfile(&manifest, &m, &lock2)
            .unwrap_err()
            .to_string();
        assert!(
            err2.contains("0.9.0") && err2.contains("1.0.0"),
            "expected root version mismatch, got {err2}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_rejects_path_dep_without_version() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_nover_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let dep = dir.join("vendor").join("leaf");
        fs::create_dir_all(&dep).unwrap();
        // Path dep exists but has no Lumia.toml and no version pin.
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"

[dependencies]
leaf = { path = "vendor/leaf" }
"#,
        )
        .unwrap();
        let m = load_manifest(&manifest).unwrap();
        let err = lock_from_manifest(&manifest, &m).unwrap_err().to_string();
        assert!(
            err.contains("no version") && err.contains("leaf"),
            "expected missing-version error, got {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_version_string_reads_dep_package_version() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_verpin_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let dep = dir.join("deps").join("leaf");
        fs::create_dir_all(&dep).unwrap();
        fs::write(
            dep.join("Lumia.toml"),
            r#"[package]
name = "leaf"
version = "2.0.0"
"#,
        )
        .unwrap();
        let manifest = dir.join("Lumia.toml");
        // Constraint string must match dep package.version (no silent fake pin).
        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"

[dependencies]
leaf = "2.0.0"
"#,
        )
        .unwrap();
        let m = load_manifest(&manifest).unwrap();
        let lock = lock_from_manifest(&manifest, &m).unwrap();
        let leaf = lock.package.iter().find(|p| p.name == "leaf").unwrap();
        assert_eq!(leaf.version, "2.0.0");

        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"

[dependencies]
leaf = "0.1"
"#,
        )
        .unwrap();
        let m = load_manifest(&manifest).unwrap();
        let err = lock_from_manifest(&manifest, &m).unwrap_err().to_string();
        assert!(
            err.contains("0.1") && err.contains("2.0.0"),
            "expected constraint vs package.version mismatch, got {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_content_fingerprint_detects_vendor_edit() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_content_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let dep = dir.join("deps").join("leaf");
        fs::create_dir_all(&dep).unwrap();
        fs::write(
            dep.join("Lumia.toml"),
            r#"[package]
name = "leaf"
version = "1.0.0"
"#,
        )
        .unwrap();
        fs::write(dep.join("Lib.lm"), "module Lib\nval x = 1\n").unwrap();
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"

[dependencies]
leaf = "1.0.0"
"#,
        )
        .unwrap();
        let m = load_manifest(&manifest).unwrap();
        let lock = lock_from_manifest(&manifest, &m).unwrap();
        let leaf = lock.package.iter().find(|p| p.name == "leaf").unwrap();
        assert!(
            !leaf.content.is_empty(),
            "dep should have content fingerprint"
        );
        verify_lockfile(&manifest, &m, &lock).unwrap();

        // Vendor edit without version bump must fail verify.
        fs::write(dep.join("Lib.lm"), "module Lib\nval x = 2\n").unwrap();
        let err = verify_lockfile(&manifest, &m, &lock)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("content") || err.contains("fingerprint"),
            "expected content mismatch, got {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lockfile_rejects_unknown_fields() {
        let err = toml::from_str::<Lockfile>(
            r#"
[[package]]
name = "root"
version = "1.0.0"
path = "."
extra = true
"#,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("extra") || msg.contains("unknown"),
            "expected unknown-field error, got {msg}"
        );
    }

    fn write_leaf_pkg(dir: &Path, name: &str, version: &str, src: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("Lumia.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n"),
        )
        .unwrap();
        fs::write(dir.join("Lib.lm"), src).unwrap();
    }

    #[test]
    fn remove_dep_missing_does_not_rewrite() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_rmiss_{}", std::process::id()));
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
        let err = remove_dep(&manifest, "nope").unwrap_err();
        assert!(
            err.to_string().contains("nope"),
            "expected missing-dep error, got {err}"
        );
        assert_eq!(before, fs::read_to_string(&manifest).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_dep_refreshes_lock() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_rm_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let leaf = dir.join("deps").join("leaf");
        write_leaf_pkg(&leaf, "leaf", "1.0.0", "module Lib\nval x = 1\n");
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"

[dependencies]
leaf = { path = "deps/leaf" }
"#,
        )
        .unwrap();
        write_lock_from_manifest(&manifest).unwrap();
        remove_dep(&manifest, "leaf").unwrap();
        let w = write_lock_from_manifest(&manifest).unwrap();
        assert!(
            w.diff.removed.iter().any(|p| p.name == "leaf"),
            "expected leaf removed from lock, got {:?}",
            w.diff
        );
        let m = load_manifest(&manifest).unwrap();
        assert!(!m.dependencies.contains_key("leaf"));
        let lock = load_lockfile(&w.path).unwrap();
        assert!(lock.package.iter().all(|p| p.name != "leaf"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn outdated_then_update_content_fingerprint() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_out_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let leaf = dir.join("deps").join("leaf");
        write_leaf_pkg(&leaf, "leaf", "1.0.0", "module Lib\nval x = 1\n");
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"

[dependencies]
leaf = { path = "deps/leaf" }
"#,
        )
        .unwrap();
        write_lock_from_manifest(&manifest).unwrap();
        let (_, fresh) = outdated_lock(&manifest).unwrap();
        assert!(fresh.is_empty(), "fresh lock should be up to date: {fresh}");
        fs::write(leaf.join("Lib.lm"), "module Lib\nval x = 2\n").unwrap();
        let (path, stale) = outdated_lock(&manifest).unwrap();
        assert!(
            stale.changed.iter().any(|c| c.name == "leaf" && c.content),
            "expected content change, got {stale}"
        );
        assert_eq!(path, lockfile_path(&manifest));
        let w = write_lock_from_manifest(&manifest).unwrap();
        assert!(!w.created);
        assert!(w.diff.changed.iter().any(|c| c.name == "leaf" && c.content));
        let (_, after) = outdated_lock(&manifest).unwrap();
        assert!(after.is_empty(), "update should clear outdated: {after}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn outdated_missing_lock_errors() {
        let dir = std::env::temp_dir().join(format!("lumia_pkg_nolock_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("Lumia.toml");
        fs::write(
            &manifest,
            r#"[package]
name = "root"
version = "1.0.0"
"#,
        )
        .unwrap();
        let err = outdated_lock(&manifest).unwrap_err().to_string();
        assert!(
            err.contains("missing") && err.contains("pkg lock"),
            "expected missing-lock hint, got {err}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_lockfiles_add_remove_version() {
        let old = Lockfile {
            package: vec![
                LockPackage {
                    name: "root".into(),
                    version: "1.0.0".into(),
                    path: ".".into(),
                    content: String::new(),
                },
                LockPackage {
                    name: "gone".into(),
                    version: "0.1.0".into(),
                    path: "deps/gone".into(),
                    content: "aaa".into(),
                },
            ],
        };
        let new = Lockfile {
            package: vec![
                LockPackage {
                    name: "root".into(),
                    version: "1.1.0".into(),
                    path: ".".into(),
                    content: String::new(),
                },
                LockPackage {
                    name: "new".into(),
                    version: "2.0.0".into(),
                    path: "deps/new".into(),
                    content: "bbb".into(),
                },
            ],
        };
        let d = diff_lockfiles(&old, &new);
        assert!(d.added.iter().any(|p| p.name == "new"));
        assert!(d.removed.iter().any(|p| p.name == "gone"));
        assert!(d
            .changed
            .iter()
            .any(|c| { c.name == "root" && c.version == Some(("1.0.0".into(), "1.1.0".into())) }));
        let text = d.to_string();
        assert!(text.contains("+ new"), "{text}");
        assert!(text.contains("- gone"), "{text}");
        assert!(text.contains("1.0.0") && text.contains("1.1.0"), "{text}");
    }
}

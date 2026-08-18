//! Lumia package manifest (`Lumia.toml`) + lockfile (`Lumia.lock`).

mod link;
mod lock;
mod manifest;

pub use link::{collect_link_args, validate_cli_link_arg};
pub use lock::{
    dependency_roots_from_lock, load_lockfile, lock_from_manifest, verify_lockfile, write_lockfile,
    LockPackage, Lockfile,
};
pub use manifest::{
    add_path_dep, dependency_roots, find_manifest, init_manifest, load_manifest, write_manifest,
    DepSpec, DepTable, Manifest, PackageMeta,
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
}

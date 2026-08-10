//! Lumia package manifest (`Lumia.toml`) + lockfile (`Lumia.lock`).

mod link;
mod lock;
mod manifest;

pub use link::{collect_link_args, validate_cli_link_arg};
pub use lock::{
    load_lockfile, lock_from_manifest, verify_lockfile, write_lockfile, LockPackage, Lockfile,
};
pub use manifest::{
    add_path_dep, dependency_roots, find_manifest, init_manifest, load_manifest, write_manifest,
    DepSpec, Manifest, PackageMeta,
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
}

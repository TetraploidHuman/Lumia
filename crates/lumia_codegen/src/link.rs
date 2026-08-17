//! Clang linking and runtime library discovery.

use anyhow::{bail, Context as AnyhowContext, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn link_executable(
    obj: &Path,
    runtime: &Path,
    output: &Path,
    extra: &[String],
    release: bool,
) -> Result<()> {
    let linker = std::env::var("LUMIA_LINKER").unwrap_or_else(|_| "clang".into());
    let mut cmd = Command::new(&linker);
    cmd.arg(obj).arg(runtime).arg("-o").arg(output);
    // `lumia_rt` is a Rust staticlib: pull in the host libs Rust std needs.
    // (Matches `cargo rustc -p lumia_rt -- --print=native-static-libs`.)
    if cfg!(target_os = "windows") {
        cmd.args([
            "-ladvapi32",
            "-lws2_32",
            "-luserenv",
            "-lbcrypt",
            "-lntdll",
            // Match the compiler binary stack (see .cargo/config.toml).
            "-Wl,/STACK:16777216",
        ]);
    } else {
        cmd.arg("-lpthread")
            .arg("-ldl")
            .arg("-lm")
            .arg("-lrt")
            .arg("-lutil");
        // Drop unused `lumia_rt` / Rust-std objects on all profiles (not only
        // Release). Cuts Debug binary size when domain kernels are unused.
        // Target OS is Linux + Windows (BUILD); macOS `-dead_strip` is kept only
        // as an experimental host escape hatch — not a supported product path.
        if cfg!(target_os = "macos") {
            cmd.arg("-Wl,-dead_strip");
        } else {
            cmd.arg("-Wl,--gc-sections");
            if release {
                cmd.arg("-Wl,-s");
            }
        }
    }
    for a in extra {
        cmd.arg(a);
    }
    let status = cmd
        .status()
        .with_context(|| format!("invoke linker `{linker}`"))?;
    if !status.success() {
        bail!("link failed with {status} (driver `{linker}`)");
    }
    Ok(())
}

/// Drop the intermediate object after a successful link unless `LUMIA_KEEP_OBJ` is set.
pub(crate) fn remove_link_object_unless_kept(obj: &Path) {
    if std::env::var_os("LUMIA_KEEP_OBJ").is_some() {
        return;
    }
    let _ = std::fs::remove_file(obj);
}

/// Locate `liblumia_rt.a` / `lumia_rt.lib` in target dir.
pub fn find_runtime_lib_prefer(target_dir: &Path, release: bool) -> Result<PathBuf> {
    let preferred = if release { "release" } else { "debug" };
    let fallback = if release { "debug" } else { "release" };
    let profiles = [preferred, fallback];
    let mut found_preferred: Option<PathBuf> = None;
    let mut found_fallback: Option<PathBuf> = None;
    for p in profiles {
        for name in ["liblumia_rt.a", "lumia_rt.lib", "lumia_rt.dll.lib"] {
            let c = target_dir.join(p).join(name);
            if c.exists() {
                if p == preferred {
                    found_preferred = Some(c);
                } else if found_fallback.is_none() {
                    found_fallback = Some(c);
                }
                break;
            }
        }
    }
    if let Some(c) = found_preferred {
        return Ok(c);
    }
    for name in ["liblumia_rt.a", "lumia_rt.lib"] {
        let c = target_dir.join(name);
        if c.exists() {
            return Ok(c);
        }
    }
    if let Some(c) = found_fallback {
        if std::env::var_os("LUMIA_ALLOW_CROSS_PROFILE_RT").is_some() {
            eprintln!(
                "warning: linking {} lumia_rt into a {} build ({}); set only when intentional",
                fallback,
                preferred,
                c.display(),
            );
            return Ok(c);
        }
        bail!(
            "liblumia_rt for profile `{preferred}` not found under {} (found {fallback} at {}); \
             run `cargo build -p lumia_rt{}` or set LUMIA_ALLOW_CROSS_PROFILE_RT=1 to override",
            target_dir.display(),
            c.display(),
            if release { " --release" } else { "" },
        );
    }
    bail!(
        "liblumia_rt.a / lumia_rt.lib not found under {} — run `cargo build -p lumia_rt` first",
        target_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::remove_link_object_unless_kept;
    use std::io::Write;

    #[test]
    fn remove_link_object_deletes_by_default() {
        let dir = std::env::temp_dir().join(format!(
            "lumia_keep_obj_test_{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let obj = dir.join("t.o");
        {
            let mut f = std::fs::File::create(&obj).expect("create");
            f.write_all(b"x").expect("write");
        }
        assert!(obj.exists());
        // Ensure keep flag is off for this process (tests run serially in CI for rt, but
        // codegen tests may parallel — only assert delete when unset).
        if std::env::var_os("LUMIA_KEEP_OBJ").is_none() {
            remove_link_object_unless_kept(&obj);
            assert!(!obj.exists(), "intermediate .o should be removed after link");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

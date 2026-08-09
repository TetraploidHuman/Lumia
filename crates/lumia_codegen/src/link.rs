//! Clang linking and runtime library discovery.

use anyhow::{bail, Context as AnyhowContext, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn link_executable(
    obj: &Path,
    runtime: &Path,
    output: &Path,
    extra: &[String],
) -> Result<()> {
    let mut cmd = Command::new("clang");
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
    }
    for a in extra {
        cmd.arg(a);
    }
    let status = cmd.status().context("invoke clang linker")?;
    if !status.success() {
        bail!("link failed with {status}");
    }
    Ok(())
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
        eprintln!(
            "warning: linking {} lumia_rt into a {} build ({}); run `cargo build -p lumia_rt{}` for a matching runtime",
            fallback,
            preferred,
            c.display(),
            if release { " --release" } else { "" },
        );
        return Ok(c);
    }
    bail!(
        "liblumia_rt.a / lumia_rt.lib not found under {} — run `cargo build -p lumia_rt` first",
        target_dir.display()
    )
}

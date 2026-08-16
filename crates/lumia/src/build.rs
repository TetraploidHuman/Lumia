//! Native build pipeline (`check` → lower → optimize → codegen → link).

use crate::check::{annotate_assert_messages, check_program};
use anyhow::{Context, Result};
use lumia_codegen::{compile_module, find_runtime_lib_prefer, CodegenOptions};
use lumia_core::{format_module, lower_hir_with_schemes};
use lumia_opt::{optimize, OptOptions};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Compile a Lumia source file to a native executable.
pub fn build_file(
    file: &Path,
    output: &Path,
    release: bool,
    memo_tf: bool,
    auto_parallel: bool,
    dense_f64_sr: bool,
    trust_foreign_pure: Option<bool>,
    link_args: Vec<String>,
    show_ir: bool,
    emit_llvm: bool,
) -> Result<()> {
    let (mut typed, loaded) = check_program(file, auto_parallel, trust_foreign_pure)?;
    annotate_assert_messages(&mut typed.module, &loaded);
    let mut core = lower_hir_with_schemes(
        &typed.module,
        &typed.fun_types,
        &typed.fun_schemes,
        &typed.type_at,
    )
    .map_err(|e| anyhow::anyhow!("core: {e}"))?;
    core.check_channel_elem_conflicts()
        .map_err(|e| anyhow::anyhow!("channel: {e}"))?;
    optimize(
        &mut core,
        &OptOptions {
            release,
            memo_tf: release && memo_tf,
            dense_f64_sr,
        },
    );
    if show_ir {
        print!("{}", format_module(&core));
    }

    ensure_runtime_built(release)?;

    let target_dir = workspace_target_dir();
    let runtime_lib = if let Ok(p) = std::env::var("LUMIA_RT_LIB") {
        let p = PathBuf::from(p);
        if !p.is_file() {
            anyhow::bail!("LUMIA_RT_LIB is not a file: {}", p.display());
        }
        p
    } else {
        find_runtime_lib_prefer(&target_dir, release)?
    };

    let mut link = link_args;
    for a in &loaded.link_args {
        if !link.iter().any(|x| x == a) {
            link.push(a.clone());
        }
    }
    compile_module(
        &core,
        &CodegenOptions {
            release,
            output: output.to_path_buf(),
            runtime_lib,
            emit_ir: emit_llvm,
            dense_f64_sr,
            link_args: link,
        },
    )?;
    Ok(())
}

/// Workspace root that contains this compiler (`…/Lumia`), baked in at build time.
/// Used so `lumia build` works outside the repo (e.g. `~/文档`) without hunting cwd.
fn compiler_workspace_root() -> PathBuf {
    crate::workspace_root(env!("CARGO_MANIFEST_DIR"))
}

fn workspace_target_dir() -> PathBuf {
    if let Ok(t) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(t);
    }
    compiler_workspace_root().join("target")
}

/// Build `lumia_rt` via cargo unless `LUMIA_RT_LIB` already points at an archive.
pub fn ensure_runtime_built(release: bool) -> Result<()> {
    if let Ok(p) = std::env::var("LUMIA_RT_LIB") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(());
        }
        anyhow::bail!(
            "LUMIA_RT_LIB set but not a file: {} (unset to build via cargo)",
            p.display()
        );
    }
    let root = compiler_workspace_root();
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&root);
    cmd.arg("build").arg("-p").arg("lumia_rt");
    if release {
        cmd.arg("--release");
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn cargo build -p lumia_rt in {}", root.display()))?;
    if !status.success() {
        anyhow::bail!("failed to build lumia_rt");
    }
    Ok(())
}

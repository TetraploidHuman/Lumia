//! Native build pipeline (`check` → lower → optimize → codegen → link).

use crate::check::check_program;
use crate::compile::compile_program_to_optimized;
use crate::options::CompileOptions;
use anyhow::{Context, Result};
use lumia_codegen::{compile_module, find_runtime_lib_prefer};
use lumia_core::format_module;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Typecheck a Lumia source file (same frontend knobs as [`build_file`]).
pub fn check_file(file: &Path, opts: &CompileOptions) -> Result<()> {
    let (auto_parallel, trust) = opts.check_knobs();
    let _ = check_program(file, auto_parallel, trust)?;
    Ok(())
}

/// Compile a Lumia source file to a native executable.
pub fn build_file(file: &Path, output: &Path, opts: &CompileOptions) -> Result<()> {
    let (core, loaded) = compile_program_to_optimized(file, opts)?;
    if opts.show_ir {
        print!("{}", format_module(&core));
    }

    ensure_runtime_built(opts.release)?;

    let target_dir = workspace_target_dir();
    let runtime_lib = if let Ok(p) = std::env::var("LUMIA_RT_LIB") {
        let p = PathBuf::from(p);
        if !p.is_file() {
            anyhow::bail!("LUMIA_RT_LIB is not a file: {}", p.display());
        }
        p
    } else {
        find_runtime_lib_prefer(&target_dir, opts.release)?
    };

    compile_module(
        &core,
        &opts.codegen(output.to_path_buf(), runtime_lib, &loaded.link_args),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::LlvmOptLevel;

    #[test]
    fn compile_options_opt_gates_memo_on_release() {
        let mut o = CompileOptions {
            release: false,
            memo_tf: true,
            ..CompileOptions::default()
        };
        assert!(!o.opt().memo_tf);
        o.release = true;
        assert!(o.opt().memo_tf);
        assert_eq!(o.opt().dense_f64_sr, o.dense_f64_sr);
    }

    #[test]
    fn compile_options_codegen_merges_package_link() {
        let o = CompileOptions {
            link_args: vec!["-lm".into()],
            emit_llvm: true,
            dense_f64_sr: true,
            release: true,
            ..CompileOptions::default()
        };
        let cg = o.codegen(
            PathBuf::from("out"),
            PathBuf::from("librt.a"),
            &["-Lvendor".into(), "-lm".into()],
        );
        assert!(cg.emit_ir);
        assert!(cg.dense_f64_sr);
        assert_eq!(
            cg.link_args,
            vec!["-lm".to_string(), "-Lvendor".to_string()]
        );
        assert_eq!(cg.llvm_opt, LlvmOptLevel::O3);
        assert_eq!(o.resolved_llvm_opt(), LlvmOptLevel::O3);
    }

    #[test]
    fn compile_options_llvm_opt_defaults_and_override() {
        let debug = CompileOptions::default();
        assert_eq!(debug.resolved_llvm_opt(), LlvmOptLevel::O1);
        let release = CompileOptions {
            release: true,
            ..CompileOptions::default()
        };
        assert_eq!(release.resolved_llvm_opt(), LlvmOptLevel::O3);
        let override_none = CompileOptions {
            release: true,
            llvm_opt: Some(LlvmOptLevel::None),
            ..CompileOptions::default()
        };
        let cg = override_none.codegen(PathBuf::from("out"), PathBuf::from("librt.a"), &[]);
        assert!(cg.release);
        assert_eq!(cg.llvm_opt, LlvmOptLevel::None);
        let debug_o3 = CompileOptions {
            llvm_opt: Some(LlvmOptLevel::O3),
            ..CompileOptions::default()
        };
        assert!(!debug_o3.release);
        assert_eq!(debug_o3.resolved_llvm_opt(), LlvmOptLevel::O3);
    }

    #[test]
    fn compile_options_check_knobs() {
        let o = CompileOptions {
            auto_parallel: false,
            trust_foreign_pure: Some(true),
            ..CompileOptions::default()
        };
        assert_eq!(o.check_knobs(), (false, Some(true)));
    }
}

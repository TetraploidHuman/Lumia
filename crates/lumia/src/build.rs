//! Native build pipeline (`check` → lower → optimize → codegen → link).

use crate::check::check_program;
use crate::compile::compile_program_to_optimized;
use anyhow::{Context, Result};
use lumia_codegen::{compile_module, find_runtime_lib_prefer, CodegenOptions};
use lumia_core::format_module;
use lumia_opt::OptOptions;
use std::path::{Path, PathBuf};
use std::process::Command;

pub use lumia_codegen::LlvmOptLevel;

/// Unified flags for the native compile pipeline (CLI / IDE Run share this).
///
/// Mid-end and codegen still take [`OptOptions`] / [`CodegenOptions`]; this type
/// is the single place that maps user-facing knobs onto those crates (Todo:
/// 编译选项仍四散). LLVM level is [`Self::llvm_opt`] → [`CodegenOptions::llvm_opt`].
#[derive(Clone, Debug)]
pub struct CompileOptions {
    pub release: bool,
    /// Transparent Memo `T_f`; effective only when [`Self::release`].
    pub memo_tf: bool,
    pub auto_parallel: bool,
    pub dense_f64_sr: bool,
    pub trust_foreign_pure: Option<bool>,
    pub link_args: Vec<String>,
    pub show_ir: bool,
    pub emit_llvm: bool,
    /// LLVM new-PM override. `None` → [`LlvmOptLevel::from_release`] (`O1` Debug / `O3` Release).
    pub llvm_opt: Option<LlvmOptLevel>,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            release: false,
            memo_tf: true,
            auto_parallel: true,
            dense_f64_sr: false,
            trust_foreign_pure: None,
            link_args: Vec::new(),
            show_ir: false,
            emit_llvm: false,
            llvm_opt: None,
        }
    }
}

impl CompileOptions {
    pub fn opt(&self) -> OptOptions {
        OptOptions {
            release: self.release,
            memo_tf: self.release && self.memo_tf,
            dense_f64_sr: self.dense_f64_sr,
            domain_sr: self.release && cfg!(feature = "codegen"),
        }
    }

    /// Frontend check knobs (`auto_parallel` + optional trust override).
    ///
    /// Kept as a pair so `check_program` can still resolve package
    /// `trust_foreign_pure` when the override is `None`.
    pub fn check_knobs(&self) -> (bool, Option<bool>) {
        (self.auto_parallel, self.trust_foreign_pure)
    }

    pub fn codegen(
        &self,
        output: PathBuf,
        runtime_lib: PathBuf,
        package_link: &[String],
    ) -> CodegenOptions {
        let mut link = self.link_args.clone();
        for a in package_link {
            if !link.iter().any(|x| x == a) {
                link.push(a.clone());
            }
        }
        CodegenOptions {
            release: self.release,
            llvm_opt: self.resolved_llvm_opt(),
            output,
            runtime_lib,
            emit_ir: self.emit_llvm,
            dense_f64_sr: self.dense_f64_sr,
            link_args: link,
        }
    }

    /// Effective LLVM level: explicit `--llvm-opt`, else Debug O1 / Release O3.
    pub fn resolved_llvm_opt(&self) -> LlvmOptLevel {
        self.llvm_opt
            .unwrap_or_else(|| LlvmOptLevel::from_release(self.release))
    }
}

/// Typecheck a Lumia source file (same frontend knobs as [`build_file`]).
pub fn check_file(file: &Path, opts: &CompileOptions) -> Result<()> {
    let (auto_parallel, trust) = opts.check_knobs();
    let _ = check_program(file, auto_parallel, trust)?;
    Ok(())
}

/// Compile a Lumia source file to a native executable.
pub fn build_file(file: &Path, output: &Path, opts: &CompileOptions) -> Result<()> {
    let (auto_parallel, trust) = opts.check_knobs();
    let (core, loaded) = compile_program_to_optimized(file, auto_parallel, trust, &opts.opt())?;
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

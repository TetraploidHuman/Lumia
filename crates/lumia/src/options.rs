//! Unified compile options — single source of truth for CLI / IDE / test knobs.
//!
//! [`CompileOptions`] is always available (even without the `codegen` feature);
//! codegen-specific fields and methods are gated behind `#[cfg(feature = "codegen")]`.

#[cfg(feature = "codegen")]
use lumia_codegen::CodegenOptions;
#[cfg(feature = "codegen")]
pub use lumia_codegen::LlvmOptLevel;
#[cfg(feature = "codegen")]
use std::path::PathBuf;

/// Unified flags for the compile pipeline (CLI / IDE Run / tests share this).
///
/// Available with or without the `codegen` feature; codegen-specific fields
/// (`llvm_opt`, `link_args`, `emit_llvm`) are `#[cfg(feature = "codegen")]`.
#[derive(Clone, Debug)]
pub struct CompileOptions {
    pub release: bool,
    /// Transparent Memo `T_f`; effective only when [`Self::release`].
    pub memo_tf: bool,
    pub auto_parallel: bool,
    pub dense_f64_sr: bool,
    pub trust_foreign_pure: Option<bool>,
    pub show_ir: bool,

    // -- codegen-only fields ---------------------------------------------------
    #[cfg(feature = "codegen")]
    pub link_args: Vec<String>,
    #[cfg(feature = "codegen")]
    pub emit_llvm: bool,
    /// LLVM new-PM override. `None` → `O1` Debug / `O3` Release.
    #[cfg(feature = "codegen")]
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
            show_ir: false,
            #[cfg(feature = "codegen")]
            link_args: Vec::new(),
            #[cfg(feature = "codegen")]
            emit_llvm: false,
            #[cfg(feature = "codegen")]
            llvm_opt: None,
        }
    }
}

impl CompileOptions {
    /// Frontend check knobs (`auto_parallel` + optional trust override).
    pub fn check_knobs(&self) -> (bool, Option<bool>) {
        (self.auto_parallel, self.trust_foreign_pure)
    }
}

// ---------------------------------------------------------------------------
// codegen-only: OptOptions fan-out + CodegenOptions fan-out + LlvmOptLevel
// ---------------------------------------------------------------------------

#[cfg(any(feature = "codegen", feature = "codegen-slim"))]
impl CompileOptions {
    /// Mid-end optimization options derived from these compile flags.
    ///
    /// Feature gates (`dense-f64-sr`, `domain-sr`, `nsw-iv`) are resolved by
    /// [`lumia_opt::OptOptions::for_build`]; this method layers user knobs
    /// (`memo_tf`, `dense_f64_sr`) on top.
    pub fn opt(&self) -> lumia_opt::OptOptions {
        let mut o = lumia_opt::OptOptions::for_build(self.release);
        o.memo_tf = self.release && self.memo_tf;
        if !self.dense_f64_sr {
            o.dense_f64_sr = false;
        }
        o
    }
}

#[cfg(feature = "codegen")]
impl CompileOptions {
    /// Build a [`CodegenOptions`] from these flags plus per-invocation paths.
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

//! Shared HIR → TypedModule frontend used by CLI, LSP, and Core IR tooling.

use crate::effects::check_effect_boundaries;
use crate::infer::{infer_module_with_options, InferOptions};
use crate::parallel::finalize_auto_parallel;
use crate::types::{NameVisibility, TypeError, TypedModule};
use lumia_hir::Module;

/// Options for the shared typecheck pipeline.
#[derive(Debug, Clone)]
pub struct TypecheckOptions {
    /// Select FunRef-safe `ListParMap` / assoc `ListParFold` (default on).
    pub auto_parallel: bool,
    /// Honor `foreign "C" pure` as pure (default off; FFI purity unverified).
    pub trust_foreign_pure: bool,
}

impl Default for TypecheckOptions {
    fn default() -> Self {
        Self {
            auto_parallel: true,
            trust_foreign_pure: false,
        }
    }
}

impl TypecheckOptions {
    pub fn with_parallel(mut self, auto_parallel: bool) -> Self {
        self.auto_parallel = auto_parallel;
        self
    }

    pub fn with_trust_foreign_pure(mut self, trust: bool) -> Self {
        self.trust_foreign_pure = trust;
        self
    }
}

/// Infer → finalize auto-parallel → check effect boundaries.
///
/// This is the single frontend typecheck path shared by CLI, LSP, and
/// [`lumia_core::pipeline`].
pub fn typecheck_hir(
    hir: &Module,
    visibility: NameVisibility,
    opts: &TypecheckOptions,
) -> Result<TypedModule, TypeError> {
    let mut typed = infer_module_with_options(
        hir,
        visibility,
        InferOptions {
            trust_foreign_pure: opts.trust_foreign_pure,
        },
    )?;
    finalize_auto_parallel(&mut typed, opts.auto_parallel);
    check_effect_boundaries(&typed)?;
    Ok(typed)
}

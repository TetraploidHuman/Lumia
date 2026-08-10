//! Shared frontend→Core pipeline for tests and tooling.
//!
//! Multi-file load, import visibility, and assert-message annotation remain
//! CLI-only ([`lumia`] crate). Effect-boundary checks mirror the CLI via
//! [`lumia_ty::typecheck_hir`].

use crate::ir::CoreModule;
use crate::lower::lower_hir_with_schemes;
use lumia_hir::lower_module;
use lumia_syntax::parse_module;
use lumia_ty::{typecheck_hir, NameVisibility, TypecheckOptions};

/// Options for the test/tooling frontend (subset of CLI `check_program`).
#[derive(Debug, Clone)]
pub struct FrontendOptions {
    /// Select FunRef-safe `ListParMap` / assoc `ListParFold` (default on).
    pub auto_parallel: bool,
    /// Honor `foreign "C" pure` as pure (default off; FFI purity unverified).
    pub trust_foreign_pure: bool,
}

impl Default for FrontendOptions {
    fn default() -> Self {
        Self {
            auto_parallel: true,
            trust_foreign_pure: false,
        }
    }
}

impl FrontendOptions {
    pub fn with_parallel(mut self, auto_parallel: bool) -> Self {
        self.auto_parallel = auto_parallel;
        self
    }

    pub fn with_trust_foreign_pure(mut self, trust: bool) -> Self {
        self.trust_foreign_pure = trust;
        self
    }
}

/// Format a staged pipeline failure (`parse: …`, `lower: …`, …).
fn stage<T, E: std::fmt::Display>(name: &str, r: Result<T, E>) -> Result<T, String> {
    r.map_err(|e| format!("{name}: {e}"))
}

/// Parse → HIR → [`typecheck_hir`] → Core (incl. mono).
///
/// Mirrors the CLI path up to (but not including) `lumia_opt::optimize`,
/// without multi-file load / visibility / assert annotation.
pub fn compile_source_to_core(src: &str) -> Result<CoreModule, String> {
    compile_source_to_core_with_options(src, &FrontendOptions::default())
}

/// Same as [`compile_source_to_core`] with explicit auto-parallel toggle.
pub fn compile_source_to_core_with_parallel(
    src: &str,
    auto_parallel: bool,
) -> Result<CoreModule, String> {
    compile_source_to_core_with_options(
        src,
        &FrontendOptions {
            auto_parallel,
            ..FrontendOptions::default()
        },
    )
}

/// Parse → HIR → typecheck (infer + parallel finalize + effects) → Core.
pub fn compile_source_to_core_with_options(
    src: &str,
    opts: &FrontendOptions,
) -> Result<CoreModule, String> {
    let ast = stage("parse", parse_module(src))?;
    let hir = stage("lower", lower_module(&ast))?;
    let typed = stage(
        "typecheck",
        typecheck_hir(
            &hir,
            NameVisibility::default(),
            &TypecheckOptions {
                auto_parallel: opts.auto_parallel,
                trust_foreign_pure: opts.trust_foreign_pure,
            },
        ),
    )?;
    Ok(lower_hir_with_schemes(
        &typed.module,
        &typed.fun_types,
        &typed.fun_schemes,
    ))
}

/// Read a `.lm` file and compile through to Core.
pub fn compile_file_to_core(path: &std::path::Path) -> Result<CoreModule, String> {
    let src = stage("read", std::fs::read_to_string(path))?;
    compile_source_to_core(&src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Op, Value};
    use lumia_hir::Builtin;

    fn has_builtin(core: &CoreModule, b: Builtin) -> bool {
        core.functions.iter().any(|f| {
            f.body.ops.iter().any(|op| match op {
                Op::Let { value, .. } | Op::Effect { value } => {
                    matches!(value, Value::Builtin { name, .. } if *name == b)
                }
                _ => false,
            })
        })
    }

    #[test]
    fn trust_foreign_pure_allows_pure_ffi() {
        let src = r#"
module M
foreign "C" pure fn add(a: Int, b: Int) -> Int
val main = { add(1, 2) }
"#;
        let err = compile_source_to_core(src).expect_err("default rejects foreign pure");
        assert!(
            err.to_lowercase().contains("pure") || err.to_lowercase().contains("trust"),
            "unexpected error: {err}"
        );
        let ok = compile_source_to_core_with_options(
            src,
            &FrontendOptions::default().with_trust_foreign_pure(true),
        );
        assert!(ok.is_ok(), "trusted foreign pure: {ok:?}");
    }

    #[test]
    fn auto_parallel_off_demotes_list_par_map() {
        let src = r#"
module M
import std.io.{println}
val main = {
    println(listOf(1, 2, 3).map({ x -> x + 1 }).len())
}
"#;
        let with_par = compile_source_to_core_with_options(
            src,
            &FrontendOptions::default().with_parallel(true),
        )
        .expect("core");
        let no_par = compile_source_to_core_with_options(
            src,
            &FrontendOptions::default().with_parallel(false),
        )
        .expect("core");
        assert!(
            !has_builtin(&no_par, Builtin::ListParMap),
            "ListParMap must be absent when auto_parallel=false"
        );
        let _ = with_par;
    }

    #[test]
    fn effect_boundaries_run_on_default_pipeline() {
        let bad = r#"
module Bad
import std.io.{println}
val xs = println(1)
val main = { 0 }
"#;
        let err = compile_source_to_core(bad).expect_err("top-level IO");
        assert!(
            err.contains("typecheck:") || err.to_lowercase().contains("effect"),
            "unexpected error: {err}"
        );
        let ok = r#"
module Ok
import std.io.{println}
val main = { println(1) }
"#;
        compile_source_to_core(ok).expect("main may perform IO");
    }
}

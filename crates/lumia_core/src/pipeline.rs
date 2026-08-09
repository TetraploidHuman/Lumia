//! Shared frontend→Core pipeline for tests and tooling.

use crate::ir::CoreModule;
use crate::lower::lower_hir_with_schemes;
use lumia_hir::lower_module;
use lumia_syntax::parse_module;
use lumia_ty::{finalize_auto_parallel, infer_module};

/// Format a staged pipeline failure (`parse: …`, `lower: …`, …).
fn stage<T, E: std::fmt::Display>(name: &str, r: Result<T, E>) -> Result<T, String> {
    r.map_err(|e| format!("{name}: {e}"))
}

/// Parse → HIR → infer → auto-parallel finalize → Core (incl. mono).
///
/// Mirrors the CLI path up to (but not including) `lumia_opt::optimize`.
pub fn compile_source_to_core(src: &str) -> Result<CoreModule, String> {
    compile_source_to_core_with_parallel(src, true)
}

/// Same as [`compile_source_to_core`] with explicit auto-parallel toggle.
pub fn compile_source_to_core_with_parallel(
    src: &str,
    auto_parallel: bool,
) -> Result<CoreModule, String> {
    let ast = stage("parse", parse_module(src))?;
    let hir = stage("lower", lower_module(&ast))?;
    let mut typed = stage("infer", infer_module(&hir))?;
    finalize_auto_parallel(&mut typed, auto_parallel);
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

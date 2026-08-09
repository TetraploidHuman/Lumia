//! Shared frontend→Core pipeline for tests and tooling.

use crate::ir::CoreModule;
use crate::lower::lower_hir_with_schemes;
use lumia_hir::lower_module;
use lumia_syntax::parse_module;
use lumia_ty::{finalize_auto_parallel, infer_module};

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
    let ast = parse_module(src).map_err(|e| e.to_string())?;
    let hir = lower_module(&ast).map_err(|e| e.to_string())?;
    let mut typed = infer_module(&hir).map_err(|e| e.to_string())?;
    finalize_auto_parallel(&mut typed, auto_parallel);
    Ok(lower_hir_with_schemes(
        &typed.module,
        &typed.fun_types,
        &typed.fun_schemes,
    ))
}

/// Read a `.lm` file and compile through to Core.
pub fn compile_file_to_core(path: &std::path::Path) -> Result<CoreModule, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    compile_source_to_core(&src)
}

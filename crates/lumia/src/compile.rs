//! Loader-backed compile helpers that continue from `check_program` into Core
//! and opt. Unlike `lumia_core::compile_source_to_core*`, these APIs honor the
//! real package/std loader and therefore work for multi-file/import fixtures.

use crate::check::check_program;
use crate::load::{path_label, LoadedProgram};
use anyhow::Result;
use lumia_core::{compile_typed_to_core, CoreModule};
use lumia_opt::{optimize, OptOptions};
use lumia_ty::TypedModule;
use std::path::Path;

/// Lower a typed, loader-resolved program to Core using the shared
/// `TypedModule -> Core` pipeline.
pub fn typed_program_to_core(typed: &TypedModule, loaded: &LoadedProgram) -> Result<CoreModule> {
    let labels: Vec<String> = loaded.files.iter().map(|f| path_label(&f.path)).collect();
    let assert_files: Vec<(&str, &str)> = labels
        .iter()
        .zip(&loaded.files)
        .map(|(label, file)| (label.as_str(), file.src.as_str()))
        .collect();
    compile_typed_to_core(typed, &assert_files).map_err(anyhow::Error::msg)
}

/// Full loader-aware frontend → Core pipeline (imports/std/package graph on).
pub fn compile_program_to_core(
    file: &Path,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
) -> Result<(CoreModule, LoadedProgram)> {
    let (typed, loaded) = check_program(file, auto_parallel, trust_foreign_pure)?;
    let core = typed_program_to_core(&typed, &loaded)?;
    Ok((core, loaded))
}

/// Full loader-aware frontend → Core → optimize pipeline.
pub fn compile_program_to_optimized(
    file: &Path,
    auto_parallel: bool,
    trust_foreign_pure: Option<bool>,
    opts: &OptOptions,
) -> Result<(CoreModule, LoadedProgram)> {
    let (mut core, loaded) = compile_program_to_core(file, auto_parallel, trust_foreign_pure)?;
    optimize(&mut core, opts);
    Ok((core, loaded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{compile_file_to_core, format_module};
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    #[test]
    fn compile_program_to_core_uses_loader_for_import_aliases() {
        let path = workspace_root().join("examples/import_as.lm");
        assert!(
            compile_file_to_core(&path).is_err(),
            "single-buffer helper should still skip loader/std"
        );
        let (core, loaded) =
            compile_program_to_core(&path, true, None).expect("loader-backed core compile");
        assert!(
            loaded.files.len() >= 2,
            "expected entry + loaded std files, got {}",
            loaded.files.len()
        );
        assert!(
            !core.functions.is_empty(),
            "loader-backed core compile should produce functions"
        );
    }

    #[test]
    fn compile_program_to_optimized_uses_loader_for_import_aliases() {
        let path = workspace_root().join("examples/import_as.lm");
        let (core, _) = compile_program_to_optimized(&path, true, None, &OptOptions::default())
            .expect("loader-backed optimize");
        let ir = format_module(&core);
        assert!(
            ir.contains("fun main"),
            "expected formatted optimized Core to contain main, got:\n{ir}"
        );
    }
}

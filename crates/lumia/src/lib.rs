//! Lumia compiler library — load, typecheck, package, LSP, and docs.
//!
//! The `lumia` binary is a thin CLI over this crate.

#[cfg(feature = "codegen")]
pub mod build;
pub mod check;
#[cfg(any(feature = "codegen", feature = "codegen-slim"))]
pub mod compile;
pub mod options;

pub use options::CompileOptions;
#[cfg(feature = "codegen")]
pub use options::LlvmOptLevel;
#[cfg(feature = "codegen")]
pub use build::check_file;
pub mod diag;
pub mod doc;
pub mod exports;
pub mod load;
pub mod lsp;
pub mod paths;
pub mod pkg;
pub mod vis;

pub use check::{
    check_program, check_program_with_overlays, check_program_with_overlays_recovering,
    check_source, check_source_recovering, OverlayCheckError, PartialCheck, PartialProgramCheck,
};
#[cfg(any(feature = "codegen", feature = "codegen-slim"))]
pub use compile::{compile_program_to_core, compile_program_to_optimized, typed_program_to_core};
pub use diag::{Diagnostic, DiagnosticKind};
pub use load::{
    load_program, load_program_with_overlays, path_in_loaded_files, resolve_ide_entry,
    LoadedProgram, SourceFile, PACKAGE_ENTRY_RELS,
};
pub use paths::{extras_dir, std_dir, workspace_root, workspace_root_canonical};

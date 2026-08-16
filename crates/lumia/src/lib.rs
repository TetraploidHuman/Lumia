//! Lumia compiler library — load, typecheck, package, LSP, and docs.
//!
//! The `lumia` binary is a thin CLI over this crate.

#[cfg(feature = "codegen")]
pub mod build;
pub mod check;
pub mod doc;
pub mod load;
pub mod lsp;
pub mod paths;
pub mod pkg;
pub mod vis;

pub use check::{
    annotate_assert_messages, check_program, check_program_with_overlays, check_source,
    check_source_recovering, OverlayCheckError, PartialCheck,
};
pub use load::{load_program, load_program_with_overlays, LoadedProgram, SourceFile};
pub use paths::{extras_dir, std_dir, workspace_root, workspace_root_canonical};

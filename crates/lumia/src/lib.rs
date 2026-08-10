//! Lumia compiler library — load, typecheck, package, LSP, and docs.
//!
//! The `lumia` binary is a thin CLI over this crate.

pub mod check;
pub mod doc;
pub mod load;
pub mod lsp;
pub mod pkg;
pub mod vis;

pub use check::{
    annotate_assert_messages, check_program, check_program_with_overlays, check_source,
    OverlayCheckError,
};
pub use load::{load_program, load_program_with_overlays, LoadedProgram, SourceFile};

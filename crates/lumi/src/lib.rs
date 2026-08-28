//! Lumi compiler library — load, typecheck, package, LSP, and docs.
//!
//! The `lumi` binary is a thin CLI over this crate.

pub mod caps;
pub mod check;
pub mod doc;
pub mod load;
pub mod lsp;
pub mod pkg;
pub mod vis;

pub use caps::{cap_info, CapInfo, CapPhase, CapabilitySet, INVENTORY as CAP_INVENTORY};
pub use check::{
    annotate_assert_messages, check_program, check_program_with_caps, check_program_with_overlays,
    check_source, check_source_recovering, OverlayCheckError, PartialCheck,
};
pub use load::{load_program, load_program_with_overlays, LoadedProgram, SourceFile};

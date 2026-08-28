//! Lumi compiler library — load, typecheck, package, LSP, and docs.
//!
//! The `lumi` binary is a thin CLI over this crate.

#[cfg(feature = "codegen")]
pub mod build;
pub mod caps;
pub mod check;
pub mod compiler_config;
pub mod doc;
pub mod load;
pub mod lsp;
pub mod pkg;
pub mod profile;
pub mod vis;

#[cfg(feature = "codegen")]
pub use build::{
    compile_prepared, compile_with_caps, compile_with_profile, prepare_with_caps,
    prepare_with_profile, BuildOptions, PreparedProgram,
};
pub use caps::{cap_info, CapInfo, CapPhase, CapabilitySet, INVENTORY as CAP_INVENTORY};
pub use compiler_config::{CompilerConfig, PassDisables, CapDisables};
pub use profile::{caps_from_cli, CompileProfile};
pub use check::{
    annotate_assert_messages, check_program, check_program_with_caps,
    check_program_with_overlays, check_program_with_profile, check_source,
    check_source_recovering, OverlayCheckError, PartialCheck,
};
pub use load::{load_program, load_program_with_overlays, LoadedProgram, SourceFile};

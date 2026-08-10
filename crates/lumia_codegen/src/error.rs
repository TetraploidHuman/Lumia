//! Fallible codegen errors. Emit helpers convert inkwell failures via [`llvm`];
//! the public compile API still surfaces [`anyhow::Error`] at the crate boundary.

use thiserror::Error;

/// Error while emitting or linking LLVM IR.
#[derive(Debug, Error)]
pub enum CodegenError {
    #[error("{0}")]
    Message(String),
    #[error("LLVM: {0}")]
    Llvm(String),
}

impl CodegenError {
    pub fn msg(m: impl Into<String>) -> Self {
        Self::Message(m.into())
    }
}

/// Map an inkwell `Result` into [`anyhow::Error`] with an `LLVM:` prefix.
///
/// Emit modules use `anyhow::Result` end-to-end; this helper is the single
/// conversion point from builder failures (avoids a parallel typed-Result stack).
#[inline]
pub(crate) fn llvm<T, E: std::fmt::Display>(r: std::result::Result<T, E>) -> anyhow::Result<T> {
    r.map_err(|e| anyhow::anyhow!("{}", CodegenError::Llvm(e.to_string())))
}

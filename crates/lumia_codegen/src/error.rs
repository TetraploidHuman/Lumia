//! Fallible codegen errors (Builder / lookup failures no longer panic).

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

pub type Result<T> = std::result::Result<T, CodegenError>;

/// Map an inkwell `Result` into [`CodegenError::Llvm`].
#[inline]
pub(crate) fn llvm<T, E: std::fmt::Display>(r: std::result::Result<T, E>) -> Result<T> {
    r.map_err(|e| CodegenError::Llvm(e.to_string()))
}

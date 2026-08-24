//! Shared located diagnostic error used across parse / lower / typecheck.

use crate::span::Span;
use thiserror::Error;

/// A diagnostic with an optional source span (dummy span = message-only).
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct LocatedError {
    pub message: String,
    pub span: Span,
}

impl LocatedError {
    pub fn at(span: Span, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    pub fn message_only(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            span: Span::dummy(),
        }
    }
}

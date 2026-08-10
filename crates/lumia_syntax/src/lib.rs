//! Hand-written lexer and recursive-descent parser for Lumia.
//! Spans are preserved for diagnostics and LSP.

mod ast;
mod diag;
mod lexer;
mod parser;
mod pretty;
mod span;
mod stamp;
mod token;

pub use ast::*;
pub use diag::{byte_to_line_col, format_diagnostic, line_starts};
pub use lexer::Lexer;
pub use parser::{parse_expr_str, parse_module, ParseError};
pub use pretty::format_module_src;
pub use span::{BytePos, Span};
pub use stamp::stamp_module;
pub use token::{StringPart, Token};

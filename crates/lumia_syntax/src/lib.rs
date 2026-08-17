//! Hand-written lexer and recursive-descent parser for Lumia.
//! Spans are preserved for diagnostics and LSP.

mod ast;
mod diag;
mod error;
mod escape;
mod lexer;
mod parser;
mod pretty;
mod span;
mod stamp;
mod token;
pub mod visit;

pub use ast::{
    BinOp, Expr, ForBinding, ForeignItem, Import, ImportNames, ImportedName, InstanceItem,
    InterpPart, Item, MatchArm, MatchCondArm, Module, Pattern, Stmt, TraitItem, TypeItem,
    TypeKind, UnOp, ValItem, Variant, VariantFields,
};
pub use diag::{
    byte_at_metric_col, byte_to_line_col, byte_to_line_col_metric, format_diagnostic,
    format_diagnostic_files, line_starts, measure_str, metric_col_at_byte, pos_to_byte_metric,
    ColumnMetric,
};
pub use error::LocatedError;
pub use lexer::Lexer;
pub use parser::{parse_expr_str, parse_module, parse_module_recovering, ParseError, ParseOutcome};
pub use pretty::{format_matches_source, format_module_src};
pub use span::{BytePos, Span};
pub use stamp::stamp_module;
pub use token::{StringPart, Token, TokenKind};

//! Core-owned arithmetic / comparison opcodes (not syntax tokens).
//!
//! Mid-end and codegen match these; lower converts [`lumia_syntax::BinOp`] /
//! [`lumia_syntax::UnOp`] at the HIR→Core boundary.

use lumia_syntax::{BinOp as SynBinOp, UnOp as SynUnOp};
use std::fmt;

/// Core binary operator (arithmetic, comparison; `And`/`Or` only as ICE leftovers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Core unary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreUnOp {
    Neg,
    Not,
}

impl fmt::Display for CoreBinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            CoreBinOp::Add => "+",
            CoreBinOp::Sub => "-",
            CoreBinOp::Mul => "*",
            CoreBinOp::Div => "/",
            CoreBinOp::Rem => "%",
            CoreBinOp::Eq => "==",
            CoreBinOp::Ne => "!=",
            CoreBinOp::Lt => "<",
            CoreBinOp::Le => "<=",
            CoreBinOp::Gt => ">",
            CoreBinOp::Ge => ">=",
            CoreBinOp::And => "and",
            CoreBinOp::Or => "or",
        };
        write!(f, "{s}")
    }
}

impl fmt::Display for CoreUnOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            CoreUnOp::Neg => "-",
            CoreUnOp::Not => "not",
        };
        write!(f, "{s}")
    }
}

impl From<SynBinOp> for CoreBinOp {
    fn from(op: SynBinOp) -> Self {
        match op {
            SynBinOp::Add => Self::Add,
            SynBinOp::Sub => Self::Sub,
            SynBinOp::Mul => Self::Mul,
            SynBinOp::Div => Self::Div,
            SynBinOp::Rem => Self::Rem,
            SynBinOp::Eq => Self::Eq,
            SynBinOp::Ne => Self::Ne,
            SynBinOp::Lt => Self::Lt,
            SynBinOp::Le => Self::Le,
            SynBinOp::Gt => Self::Gt,
            SynBinOp::Ge => Self::Ge,
            SynBinOp::And => Self::And,
            SynBinOp::Or => Self::Or,
        }
    }
}

impl From<SynUnOp> for CoreUnOp {
    fn from(op: SynUnOp) -> Self {
        match op {
            SynUnOp::Neg => Self::Neg,
            SynUnOp::Not => Self::Not,
        }
    }
}

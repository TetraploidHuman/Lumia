//! Recursive-descent parser for a practical Lumia subset.

use crate::lexer::Lexer;
use crate::span::Span;
use crate::token::{StringPart, Token, TokenKind};
use crate::{
    BinOp, Expr, ForBinding, Import, ImportNames, ImportedName, InterpPart, Item, MatchArm,
    MatchCondArm, Module, Pattern, Stmt, TypeItem, TypeKind, UnOp, ValItem, Variant, VariantFields,
};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_module(src: &str) -> Result<Module, ParseError> {
    let mut p = Parser::new(src);
    p.parse_module()
}

/// Parse a single expression (used for `${…}` interpolation bodies).
pub fn parse_expr_str(src: &str) -> Result<Expr, ParseError> {
    let mut p = Parser::new(src);
    let e = p.parse_expr()?;
    if !p.at(&TokenKind::Eof) {
        return Err(p.error("trailing tokens in interpolation expression"));
    }
    Ok(e)
}

struct Parser<'a> {
    src: &'a str,
    lexer: Lexer<'a>,
    cur: Token,
    /// When false, `{` is not consumed as a trailing closure (e.g. `for x in xs {`).
    allow_trailing_closure: bool,
}

mod block;
mod expr;
mod items;
mod pattern;
mod util;

#[cfg(test)]
mod tests;

struct Checkpoint {
    cur: Token,
    pos: usize,
}

impl<'a> Parser<'a> {
    pub(super) fn new(src: &'a str) -> Self {
        let mut lexer = Lexer::new(src);
        let cur = lexer.next_token();
        Self {
            src,
            lexer,
            cur,
            allow_trailing_closure: true,
        }
    }

    pub(super) fn bump(&mut self) -> Token {
        let prev = self.cur.clone();
        self.cur = self.lexer.next_token();
        prev
    }

    pub(super) fn peek(&self) -> &TokenKind {
        &self.cur.kind
    }

    pub(super) fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.cur.kind) == std::mem::discriminant(kind)
    }

    /// True if the source slice between two byte positions contains a newline.
    pub(super) fn newline_between(
        &self,
        from: crate::span::BytePos,
        to: crate::span::BytePos,
    ) -> bool {
        let a = from.0 as usize;
        let b = to.0 as usize;
        if a >= b || b > self.src.len() {
            return false;
        }
        self.src[a..b].contains('\n')
    }

    /// After seeing `TypeName {`, true if next tokens are `ident =` (struct lit vs trailing closure).
    pub(super) fn looks_like_struct_lit(&self) -> bool {
        let kinds = self.lexer.peek_kinds(2);
        matches!(
            (kinds.first(), kinds.get(1)),
            (Some(TokenKind::Ident(_)), Some(TokenKind::Eq))
        )
    }
    pub(super) fn error(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            span: self.cur.span,
        }
    }

    pub(super) fn expect(&mut self, kind: TokenKind) -> Result<Token, ParseError> {
        if std::mem::discriminant(&self.cur.kind) == std::mem::discriminant(&kind) {
            Ok(self.bump())
        } else {
            Err(self.error(format!("expected {kind:?}, found {:?}", self.cur.kind)))
        }
    }

    pub(super) fn expect_ident(&mut self) -> Result<(String, Span), ParseError> {
        match &self.cur.kind {
            TokenKind::Ident(s) => {
                let s = s.clone();
                let span = self.cur.span;
                self.bump();
                Ok((s, span))
            }
            _ => Err(self.error("expected identifier")),
        }
    }
    pub(super) fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            cur: self.cur.clone(),
            // We need lexer position — extend Lexer
            pos: self.lexer.pos(),
        }
    }

    pub(super) fn restore(&mut self, cp: Checkpoint) {
        self.cur = cp.cur;
        self.lexer.set_pos(cp.pos);
    }
}

//! Recursive-descent parser for a practical Lumia subset.

use crate::error::LocatedError;
use crate::lexer::Lexer;
use crate::span::Span;
use crate::token::{StringPart, Token, TokenKind};
use crate::{
    BinOp, Expr, ForBinding, Import, ImportNames, ImportedName, InterpPart, Item, MatchArm,
    MatchCondArm, Module, Pattern, Stmt, TypeItem, TypeKind, UnOp, ValItem, Variant, VariantFields,
};

/// Parse failure (same shape as lower / typecheck diagnostics).
pub type ParseError = LocatedError;

/// Partial parse result: recovered items plus every error encountered.
#[derive(Debug, Clone)]
pub struct ParseOutcome {
    pub module: Module,
    pub errors: Vec<ParseError>,
}

/// Strict parse: fails on the first error (CLI / load).
pub fn parse_module(src: &str) -> Result<Module, ParseError> {
    let out = parse_module_recovering(src);
    if let Some(e) = out.errors.into_iter().next() {
        Err(e)
    } else {
        Ok(out.module)
    }
}

/// Parse with item-level error recovery for IDE / LSP.
///
/// After a failed item (or import), tokens are skipped until the next
/// top-level starter (`val` / `type` / …). Later items are still returned.
pub fn parse_module_recovering(src: &str) -> ParseOutcome {
    let mut p = Parser::new(src);
    p.parse_module_recovering()
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

    /// Top-level declaration / import starters used as sync points.
    pub(super) fn is_item_start(&self) -> bool {
        matches!(
            self.cur.kind,
            TokenKind::Priv
                | TokenKind::Val
                | TokenKind::Type
                | TokenKind::Trait
                | TokenKind::Instance
                | TokenKind::Foreign
                | TokenKind::Import
        )
    }

    /// True when the current token looks like a **column-0** top-level item.
    /// Used to stop unclosed `{ …` from swallowing the next declaration.
    pub(super) fn at_column0_item_start(&self) -> bool {
        if !self.is_item_start() {
            return false;
        }
        let start = self.cur.span.start.0 as usize;
        if start > self.src.len() {
            return false;
        }
        let line_start = self.src[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
        self.src[line_start..start].is_empty()
    }

    /// Skip junk until the next item/import starter (or EOF).
    /// Does not consume a token that already looks like a starter.
    pub(super) fn synchronize_item(&mut self) {
        if self.is_item_start() || self.at(&TokenKind::Eof) {
            return;
        }
        let mut depth = 0i32;
        while !self.at(&TokenKind::Eof) {
            match &self.cur.kind {
                TokenKind::LBrace => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::RBrace => {
                    if depth > 0 {
                        depth -= 1;
                    }
                    self.bump();
                }
                _ if depth == 0 && self.is_item_start() => return,
                _ => {
                    self.bump();
                }
            }
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

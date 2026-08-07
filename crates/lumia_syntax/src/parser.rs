//! Recursive-descent parser for a practical Lumia subset.

use crate::lexer::Lexer;
use crate::span::Span;
use crate::token::{StringPart, Token, TokenKind};
use crate::{
    BinOp, Expr, ForBinding, Import, ImportNames, InterpPart, Item, MatchArm, MatchCondArm, Module,
    Pattern, Stmt, TypeItem, TypeKind, UnOp, ValItem, Variant, VariantFields,
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

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        let mut lexer = Lexer::new(src);
        let cur = lexer.next_token();
        Self {
            src,
            lexer,
            cur,
            allow_trailing_closure: true,
        }
    }

    fn bump(&mut self) -> Token {
        let prev = self.cur.clone();
        self.cur = self.lexer.next_token();
        prev
    }

    fn peek(&self) -> &TokenKind {
        &self.cur.kind
    }

    fn at(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(&self.cur.kind) == std::mem::discriminant(kind)
    }

    /// True if the source slice between two byte positions contains a newline.
    fn newline_between(&self, from: crate::span::BytePos, to: crate::span::BytePos) -> bool {
        let a = from.0 as usize;
        let b = to.0 as usize;
        if a >= b || b > self.src.len() {
            return false;
        }
        self.src[a..b].contains('\n')
    }

    /// After seeing `TypeName {`, true if next tokens are `ident =` (struct lit vs trailing closure).
    fn looks_like_struct_lit(&self) -> bool {
        let kinds = self.lexer.peek_kinds(2);
        matches!(
            (kinds.first(), kinds.get(1)),
            (Some(TokenKind::Ident(_)), Some(TokenKind::Eq))
        )
    }

    fn parse_field_inits(&mut self) -> Result<Vec<(String, Expr)>, ParseError> {
        let mut fields = vec![];
        if self.at(&TokenKind::RBrace) {
            return Ok(fields);
        }
        loop {
            let (name, _) = self.expect_ident()?;
            self.expect(TokenKind::Eq)?;
            let val = self.parse_expr()?;
            fields.push((name, val));
            if self.at(&TokenKind::Comma) {
                self.bump();
                if self.at(&TokenKind::RBrace) {
                    break;
                }
                continue;
            }
            break;
        }
        Ok(fields)
    }

    fn error(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            span: self.cur.span,
        }
    }

    fn expect(&mut self, kind: TokenKind) -> Result<Token, ParseError> {
        if std::mem::discriminant(&self.cur.kind) == std::mem::discriminant(&kind) {
            Ok(self.bump())
        } else {
            Err(self.error(format!("expected {kind:?}, found {:?}", self.cur.kind)))
        }
    }

    fn expect_ident(&mut self) -> Result<(String, Span), ParseError> {
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

    fn parse_module(&mut self) -> Result<Module, ParseError> {
        let start = self.cur.span;
        self.expect(TokenKind::Module)?;
        let (name, _) = self.expect_ident()?;
        // allow dotted module names: math.vector
        let mut full = name;
        while self.at(&TokenKind::Dot) {
            self.bump();
            let (n, _) = self.expect_ident()?;
            full.push('.');
            full.push_str(&n);
        }

        let mut imports = vec![];
        while self.at(&TokenKind::Import) {
            imports.push(self.parse_import()?);
        }

        let mut items = vec![];
        while !self.at(&TokenKind::Eof) {
            items.push(self.parse_item()?);
        }

        Ok(Module {
            name: full,
            span: start.merge(self.cur.span),
            imports,
            items,
        })
    }

    fn parse_import(&mut self) -> Result<Import, ParseError> {
        let start = self.bump().span; // import
        let mut path = vec![];
        let (first, _) = self.expect_ident()?;
        path.push(first);
        while self.at(&TokenKind::Dot) {
            self.bump();
            match &self.cur.kind {
                TokenKind::LBrace => break,
                TokenKind::Star => break,
                TokenKind::Ident(_) => {
                    let (n, _) = self.expect_ident()?;
                    path.push(n);
                }
                _ => return Err(self.error("expected ident, `{`, or `*` after `.` in import")),
            }
        }

        let names = if self.at(&TokenKind::LBrace) {
            self.bump();
            let mut ns = vec![];
            loop {
                let (n, _) = self.expect_ident()?;
                // optional `as`
                if self.at(&TokenKind::As) {
                    self.bump();
                    let (alias, _) = self.expect_ident()?;
                    ns.push(alias); // simplify: use alias as imported name
                    let _ = n;
                } else {
                    ns.push(n);
                }
                if self.at(&TokenKind::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
            self.expect(TokenKind::RBrace)?;
            // path was a.b.{c} so last segment before brace is parent
            ImportNames::Selective(ns)
        } else if self.at(&TokenKind::Star) {
            self.bump();
            ImportNames::All
        } else {
            let last = path.pop().unwrap();
            ImportNames::Single(last)
        };

        Ok(Import {
            path,
            names,
            span: start.merge(self.cur.span),
        })
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        let is_priv = if self.at(&TokenKind::Priv) {
            self.bump();
            true
        } else {
            false
        };
        if self.at(&TokenKind::Foreign) {
            if is_priv {
                return Err(self.error("`priv foreign` is not supported"));
            }
            return self.parse_foreign_item();
        }
        if self.at(&TokenKind::Val) {
            let mut v = self.parse_val_item()?;
            v.is_priv = is_priv;
            Ok(Item::Val(v))
        } else if self.at(&TokenKind::Type) {
            let mut t = self.parse_type_item()?;
            t.is_priv = is_priv;
            Ok(Item::Type(t))
        } else if matches!(
            self.cur.kind,
            TokenKind::Trait | TokenKind::Instance | TokenKind::Requires
        ) {
            Err(self.error(
                "`trait` / `instance` / `requires` are reserved but not implemented yet (DESIGN §3.6 / §8.2)",
            ))
        } else {
            Err(self.error("expected `val`, `type`, or `foreign` item"))
        }
    }

    /// `foreign "C" [pure] fn name(x: Int, y: Int) -> Int`
    fn parse_foreign_item(&mut self) -> Result<Item, ParseError> {
        let start = self.bump().span; // foreign
        let abi = match &self.cur.kind {
            TokenKind::String(s) => {
                let s = s.clone();
                self.bump();
                s
            }
            _ => return Err(self.error("expected ABI string after `foreign` (e.g. \"C\")")),
        };
        let is_pure = if matches!(self.cur.kind, TokenKind::Ident(ref s) if s == "pure") {
            self.bump();
            true
        } else {
            false
        };
        let (kw, _) = self.expect_ident()?;
        if kw != "fn" {
            return Err(self.error("expected `fn` after foreign ABI"));
        }
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::LParen)?;
        let mut params = vec![];
        if !self.at(&TokenKind::RParen) {
            loop {
                let (pname, _) = self.expect_ident()?;
                self.expect(TokenKind::Colon)?;
                let (pty, _) = self.expect_ident()?;
                params.push((pname, pty));
                if self.at(&TokenKind::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Arrow)?;
        let (ret, ret_span) = self.expect_ident()?;
        Ok(Item::Foreign(crate::ForeignItem {
            abi,
            name,
            params,
            ret,
            is_pure,
            span: start.merge(ret_span),
        }))
    }

    fn parse_val_item(&mut self) -> Result<ValItem, ParseError> {
        let start = self.bump().span; // val
        let (name, _) = self.expect_ident()?;
        let params = if self.at(&TokenKind::LParen) {
            self.bump();
            let mut ps = vec![];
            if !self.at(&TokenKind::RParen) {
                loop {
                    let (p, _) = self.expect_ident()?;
                    ps.push(p);
                    if self.at(&TokenKind::Comma) {
                        self.bump();
                        continue;
                    }
                    break;
                }
            }
            self.expect(TokenKind::RParen)?;
            Some(ps)
        } else {
            None
        };
        self.expect(TokenKind::Eq)?;
        let body = self.parse_expr()?;
        let span = start.merge(body.span());
        Ok(ValItem {
            name,
            params,
            body,
            span,
            is_priv: false,
        })
    }

    fn parse_type_item(&mut self) -> Result<TypeItem, ParseError> {
        let start = self.bump().span;
        let (name, _) = self.expect_ident()?;
        self.expect(TokenKind::LBrace)?;
        // Peek first member to decide product vs sum
        let kind = if self.at(&TokenKind::Val) {
            let mut fields = vec![];
            while self.at(&TokenKind::Val) {
                self.bump();
                let (f, _) = self.expect_ident()?;
                fields.push(f);
                if self.at(&TokenKind::Comma) {
                    self.bump();
                }
            }
            TypeKind::Product(fields)
        } else {
            let mut variants = vec![];
            while !self.at(&TokenKind::RBrace) {
                let (vname, _) = self.expect_ident()?;
                let fields = if self.at(&TokenKind::LParen) {
                    self.bump();
                    let mut n = 0;
                    if !self.at(&TokenKind::RParen) {
                        loop {
                            let _ = self.expect_ident()?;
                            n += 1;
                            if self.at(&TokenKind::Comma) {
                                self.bump();
                                continue;
                            }
                            break;
                        }
                    }
                    self.expect(TokenKind::RParen)?;
                    VariantFields::Positional(n)
                } else if self.at(&TokenKind::LBrace) {
                    self.bump();
                    let mut named = vec![];
                    while self.at(&TokenKind::Val) {
                        self.bump();
                        let (f, _) = self.expect_ident()?;
                        named.push(f);
                        if self.at(&TokenKind::Comma) {
                            self.bump();
                        }
                    }
                    self.expect(TokenKind::RBrace)?;
                    VariantFields::Named(named)
                } else {
                    VariantFields::Unit
                };
                variants.push(Variant {
                    name: vname,
                    fields,
                });
                if self.at(&TokenKind::Comma) {
                    self.bump();
                }
            }
            TypeKind::Sum(variants)
        };
        let end = self.expect(TokenKind::RBrace)?;
        Ok(TypeItem {
            name,
            kind,
            span: start.merge(end.span),
            is_priv: false,
        })
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_pipeline()
    }

    fn parse_pipeline(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_or()?;
        while self.at(&TokenKind::PipePipe) {
            let _ = self.bump();
            let right = self.parse_or()?;
            let span = left.span().merge(right.span());
            left = Expr::Pipeline {
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        // infix match: expr match { ... } — same line only (newline `match {` is subjectless).
        if self.at(&TokenKind::Match)
            && !self.newline_between(left.span().end, self.cur.span.start)
        {
            left = self.parse_match_suffix(left)?;
        }
        Ok(left)
    }

    fn parse_match_suffix(&mut self, scrutinee: Expr) -> Result<Expr, ParseError> {
        self.bump(); // match
        self.expect(TokenKind::LBrace)?;
        let mut arms = vec![];
        while !self.at(&TokenKind::RBrace) {
            arms.push(self.parse_match_arm()?);
        }
        let end = self.expect(TokenKind::RBrace)?;
        let span = scrutinee.span().merge(end.span);
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span,
        })
    }

    /// Kotlin-style `match { cond -> body; _ -> body }`.
    fn parse_match_cond(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(TokenKind::Match)?.span;
        self.expect(TokenKind::LBrace)?;
        let mut arms = vec![];
        while !self.at(&TokenKind::RBrace) {
            arms.push(self.parse_match_cond_arm()?);
        }
        let end = self.expect(TokenKind::RBrace)?;
        Ok(Expr::MatchCond {
            arms,
            span: start.merge(end.span),
        })
    }

    fn parse_match_cond_arm(&mut self) -> Result<MatchCondArm, ParseError> {
        let start = self.cur.span;
        let cond = if self.at(&TokenKind::Underscore) {
            self.bump();
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.expect(TokenKind::Arrow)?;
        let body = if self.at(&TokenKind::LBrace) {
            self.parse_block_expr()?
        } else {
            self.parse_expr()?
        };
        Ok(MatchCondArm {
            cond,
            body: body.clone(),
            span: start.merge(body.span()),
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
        let start = self.cur.span;
        let pattern = self.parse_pattern()?;
        // or-patterns: pattern, pattern
        let pattern = if self.at(&TokenKind::Comma) {
            let mut ps = vec![pattern];
            while self.at(&TokenKind::Comma) {
                // careful: could be confusing with other commas; only before ->
                // Lookahead: if next after comma is `->` or `if`, stop — actually or-pattern continues with pattern
                self.bump();
                // If we see `if` or `->`, we went too far — but those aren't after comma in or-pattern
                if self.at(&TokenKind::Arrow) || self.at(&TokenKind::If) {
                    return Err(self.error("dangling comma in match arm"));
                }
                ps.push(self.parse_pattern()?);
            }
            let span = start.merge(ps.last().unwrap().span());
            Pattern::Or(ps, span)
        } else {
            pattern
        };
        let guard = if self.at(&TokenKind::If) {
            self.bump();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(TokenKind::Arrow)?;
        // Arm body: `{ ... }` block, or a single expression (braces optional).
        let body = if self.at(&TokenKind::LBrace) {
            self.parse_block_expr()?
        } else {
            self.parse_expr()?
        };
        Ok(MatchArm {
            pattern,
            guard,
            body: body.clone(),
            span: start.merge(body.span()),
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let start = self.cur.span;
        match &self.cur.kind {
            TokenKind::Underscore => {
                let s = self.bump().span;
                Ok(Pattern::Wildcard(s))
            }
            TokenKind::Int(n) => {
                let n = *n;
                let s = self.bump().span;
                Ok(Pattern::Int(n, s))
            }
            TokenKind::LBracket => {
                self.bump();
                let mut elems = vec![];
                let mut rest = None;
                if !self.at(&TokenKind::RBracket) {
                    loop {
                        if self.at(&TokenKind::DotDot) {
                            self.bump();
                            let (name, _) = self.expect_ident()?;
                            rest = Some(name);
                            break;
                        }
                        elems.push(self.parse_pattern()?);
                        if self.at(&TokenKind::Comma) {
                            self.bump();
                            continue;
                        }
                        break;
                    }
                }
                let end = self.expect(TokenKind::RBracket)?;
                Ok(Pattern::List {
                    elems,
                    rest,
                    span: start.merge(end.span),
                })
            }
            TokenKind::LParen => {
                let start = self.bump().span;
                let first = self.parse_pattern()?;
                if self.at(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    while self.at(&TokenKind::Comma) {
                        self.bump();
                        if self.at(&TokenKind::RParen) {
                            break;
                        }
                        elems.push(self.parse_pattern()?);
                    }
                    let end = self.expect(TokenKind::RParen)?;
                    Ok(Pattern::Tuple {
                        elems,
                        span: start.merge(end.span),
                    })
                } else {
                    self.expect(TokenKind::RParen)?;
                    Ok(first)
                }
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                let s = self.bump().span;
                if self.at(&TokenKind::LParen) {
                    self.bump();
                    let mut args = vec![];
                    if !self.at(&TokenKind::RParen) {
                        loop {
                            args.push(self.parse_pattern()?);
                            if self.at(&TokenKind::Comma) {
                                self.bump();
                                continue;
                            }
                            break;
                        }
                    }
                    let end = self.expect(TokenKind::RParen)?;
                    Ok(Pattern::Variant {
                        name,
                        args,
                        span: s.merge(end.span),
                    })
                } else if self.at(&TokenKind::LBrace) {
                    self.bump();
                    let fields = self.parse_struct_pattern_fields()?;
                    let end = self.expect(TokenKind::RBrace)?;
                    Ok(Pattern::Struct {
                        name,
                        fields,
                        span: s.merge(end.span),
                    })
                } else {
                    Ok(Pattern::Ident(name, s))
                }
            }
            _ => Err(self.error("expected pattern")),
        }
    }

    /// `x` | `x = 0` | `x = _` inside struct patterns.
    fn parse_struct_pattern_fields(&mut self) -> Result<Vec<(String, Pattern)>, ParseError> {
        let mut fields = vec![];
        if self.at(&TokenKind::RBrace) {
            return Ok(fields);
        }
        loop {
            let (fname, fspan) = self.expect_ident()?;
            let pat = if self.at(&TokenKind::Eq) {
                self.bump();
                self.parse_pattern()?
            } else {
                Pattern::Ident(fname.clone(), fspan)
            };
            fields.push((fname, pat));
            if self.at(&TokenKind::Comma) {
                self.bump();
                if self.at(&TokenKind::RBrace) {
                    break;
                }
                continue;
            }
            break;
        }
        Ok(fields)
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.at(&TokenKind::Or) {
            self.bump();
            let right = self.parse_and()?;
            let span = left.span().merge(right.span());
            left = Expr::Binary {
                op: BinOp::Or,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_cmp()?;
        while self.at(&TokenKind::And) {
            self.bump();
            let right = self.parse_cmp()?;
            let span = left.span().merge(right.span());
            left = Expr::Binary {
                op: BinOp::And,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_range()?;
        loop {
            let op = match self.peek() {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::Ne => BinOp::Ne,
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Le => BinOp::Le,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::Ge => BinOp::Ge,
                _ => break,
            };
            self.bump();
            let right = self.parse_range()?;
            let span = left.span().merge(right.span());
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// `a..b` → `range(a, b)`; `a..=b` → `rangeInclusive(a, b)` (DESIGN §3.5.2).
    fn parse_range(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        let inclusive = if self.at(&TokenKind::DotDotEq) {
            true
        } else if self.at(&TokenKind::DotDot) {
            false
        } else {
            return Ok(left);
        };
        self.bump();
        let right = self.parse_add()?;
        let span = left.span().merge(right.span());
        let name = if inclusive {
            "rangeInclusive"
        } else {
            "range"
        };
        Ok(Expr::Call {
            callee: Box::new(Expr::Ident(name.into(), span)),
            args: vec![left, right],
            span,
        })
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_to()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let right = self.parse_to()?;
            let span = left.span().merge(right.span());
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    /// Infix `a to b` → `to(a, b)` (DESIGN §3.5.2 mapOf sugar).
    fn parse_to(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        while matches!(self.peek(), TokenKind::Ident(name) if name == "to") {
            let to_span = self.bump().span;
            let right = self.parse_mul()?;
            let span = left.span().merge(right.span());
            left = Expr::Call {
                callee: Box::new(Expr::Ident("to".into(), to_span)),
                args: vec![left, right],
                span,
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Rem,
                _ => break,
            };
            self.bump();
            let right = self.parse_unary()?;
            let span = left.span().merge(right.span());
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        if self.at(&TokenKind::Not) {
            let start = self.bump().span;
            let expr = self.parse_unary()?;
            Ok(Expr::Unary {
                op: UnOp::Not,
                span: start.merge(expr.span()),
                expr: Box::new(expr),
            })
        } else if self.at(&TokenKind::Minus) {
            let start = self.bump().span;
            let expr = self.parse_unary()?;
            Ok(Expr::Unary {
                op: UnOp::Neg,
                span: start.merge(expr.span()),
                expr: Box::new(expr),
            })
        } else {
            self.parse_postfix()
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.at(&TokenKind::LParen) {
                // call
                self.bump();
                let mut args = vec![];
                if !self.at(&TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.at(&TokenKind::Comma) {
                            self.bump();
                            continue;
                        }
                        break;
                    }
                }
                let end = self.expect(TokenKind::RParen)?;
                // trailing closure after call — disabled in for-in heads etc.
                if self.allow_trailing_closure && self.at(&TokenKind::LBrace) {
                    let clo = self.parse_lambda_or_block()?;
                    args.push(clo);
                }
                let span = expr.span().merge(
                    args
                        .last()
                        .map(|a| a.span())
                        .unwrap_or(end.span),
                );
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args,
                    span,
                };
            } else if self.allow_trailing_closure
                && self.at(&TokenKind::LBrace)
                && matches!(expr, Expr::Ident(_, _))
                && self.looks_like_struct_lit()
            {
                // Same gate as trailing closures: in `for x in xs {` / `if c {`, the `{`
                // starts the statement body — not `xs { field = ... }` struct sugar.
                let name = match &expr {
                    Expr::Ident(n, _) => n.clone(),
                    _ => unreachable!(),
                };
                let start = expr.span();
                self.bump(); // {
                let fields = self.parse_field_inits()?;
                let end = self.expect(TokenKind::RBrace)?;
                expr = Expr::StructLit {
                    name,
                    fields,
                    span: start.merge(end.span),
                };
            } else if self.allow_trailing_closure
                && self.at(&TokenKind::LBrace)
                && matches!(expr, Expr::Ident(_, _) | Expr::Field { .. })
            {
                // trailing closure without (): `xs.map { ... }` or UFCS already in field
                // Only treat as trailing call if previous was Ident/Field (callee)
                let clo = self.parse_lambda_or_block()?;
                let span = expr.span().merge(clo.span());
                expr = Expr::Call {
                    callee: Box::new(expr),
                    args: vec![clo],
                    span,
                };
            } else if self.at(&TokenKind::With) {
                self.bump();
                self.expect(TokenKind::LBrace)?;
                let fields = self.parse_field_inits()?;
                let end = self.expect(TokenKind::RBrace)?;
                let span = expr.span().merge(end.span);
                expr = Expr::With {
                    base: Box::new(expr),
                    fields,
                    span,
                };
            } else if self.at(&TokenKind::Dot) {
                self.bump();
                let (field, fspan) = match &self.cur.kind {
                    TokenKind::Ident(s) => {
                        let s = s.clone();
                        let span = self.cur.span;
                        self.bump();
                        (s, span)
                    }
                    // Tuple projection: `p.0`, `p.1`, …
                    TokenKind::Int(n) => {
                        if *n < 0 {
                            return Err(self.error("tuple field index must be non-negative"));
                        }
                        let s = n.to_string();
                        let span = self.cur.span;
                        self.bump();
                        (s, span)
                    }
                    _ => {
                        return Err(self.error("expected field name or tuple index after `.`"));
                    }
                };
                let span = expr.span().merge(fspan);
                expr = Expr::Field {
                    base: Box::new(expr),
                    field,
                    span,
                };
            } else if self.at(&TokenKind::LBracket)
                && !self.newline_between(expr.span().end, self.cur.span.start)
            {
                // index sugar: xs[i] — same-line only so `0\n[h, ..]` is next match arm
                self.bump();
                let idx = self.parse_expr()?;
                let end = self.expect(TokenKind::RBracket)?;
                let span = expr.span().merge(end.span);
                let get = Expr::Field {
                    base: Box::new(expr),
                    field: "get".into(),
                    span,
                };
                expr = Expr::Call {
                    callee: Box::new(get),
                    args: vec![idx],
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match &self.cur.kind {
            TokenKind::Int(n) => {
                let n = *n;
                let s = self.bump().span;
                Ok(Expr::Int(n, s))
            }
            TokenKind::Float(n) => {
                let n = *n;
                let s = self.bump().span;
                Ok(Expr::Float(n, s))
            }
            TokenKind::String(s) => {
                let s = s.clone();
                let sp = self.bump().span;
                Ok(Expr::String(s, sp))
            }
            TokenKind::InterpString(parts) => {
                let parts = parts.clone();
                let sp = self.bump().span;
                self.parse_interp_parts(parts, sp)
            }
            TokenKind::Char(c) => {
                let c = *c;
                let sp = self.bump().span;
                Ok(Expr::Char(c, sp))
            }
            TokenKind::True => {
                let s = self.bump().span;
                Ok(Expr::Bool(true, s))
            }
            TokenKind::False => {
                let s = self.bump().span;
                Ok(Expr::Bool(false, s))
            }
            TokenKind::Ident(name) => {
                let name = name.clone();
                let s = self.bump().span;
                Ok(Expr::Ident(name, s))
            }
            TokenKind::If => self.parse_if(),
            TokenKind::Match => self.parse_match_cond(),
            // `effect { … }` — visual effect region; same as a block (DESIGN §2.2.1).
            TokenKind::Effect => {
                self.bump();
                self.parse_lambda_or_block()
            }
            TokenKind::Trait | TokenKind::Instance | TokenKind::Requires => Err(self.error(
                "`trait` / `instance` / `requires` are reserved but not implemented yet",
            )),
            TokenKind::LBrace => self.parse_lambda_or_block(),
            TokenKind::LParen => {
                let start = self.bump().span;
                let first = self.parse_expr()?;
                if self.at(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    while self.at(&TokenKind::Comma) {
                        self.bump();
                        if self.at(&TokenKind::RParen) {
                            break;
                        }
                        elems.push(self.parse_expr()?);
                    }
                    let end = self.expect(TokenKind::RParen)?;
                    Ok(Expr::TupleLit {
                        elems,
                        span: start.merge(end.span),
                    })
                } else {
                    self.expect(TokenKind::RParen)?;
                    Ok(first)
                }
            }
            TokenKind::LBracket => {
                let start = self.bump().span;
                if self.at(&TokenKind::RBracket) {
                    let end = self.bump().span;
                    return Ok(Expr::ListLit {
                        elems: vec![],
                        span: start.merge(end),
                    });
                }
                // Empty map `[:]`
                if self.at(&TokenKind::Colon) {
                    self.bump();
                    let end = self.expect(TokenKind::RBracket)?;
                    return Ok(Expr::Call {
                        callee: Box::new(Expr::Ident("mapOf".into(), start)),
                        args: vec![],
                        span: start.merge(end.span),
                    });
                }
                let first = self.parse_expr()?;
                // Map literal `[k : v, …]` → `mapOf(k to v, …)`
                if self.at(&TokenKind::Colon) {
                    self.bump();
                    let v0 = self.parse_expr()?;
                    let mut args = vec![Self::map_pair_to(first, v0)];
                    while self.at(&TokenKind::Comma) {
                        self.bump();
                        if self.at(&TokenKind::RBracket) {
                            break;
                        }
                        let k = self.parse_expr()?;
                        self.expect(TokenKind::Colon)?;
                        let v = self.parse_expr()?;
                        args.push(Self::map_pair_to(k, v));
                    }
                    let end = self.expect(TokenKind::RBracket)?;
                    return Ok(Expr::Call {
                        callee: Box::new(Expr::Ident("mapOf".into(), start)),
                        args,
                        span: start.merge(end.span),
                    });
                }
                let mut elems = vec![first];
                while self.at(&TokenKind::Comma) {
                    self.bump();
                    if self.at(&TokenKind::RBracket) {
                        break;
                    }
                    elems.push(self.parse_expr()?);
                }
                let end = self.expect(TokenKind::RBracket)?;
                Ok(Expr::ListLit {
                    elems,
                    span: start.merge(end.span),
                })
            }
            // Set literal `#{}` / `#{a, b}` → `setOf(…)`
            TokenKind::Hash => {
                let start = self.bump().span;
                self.expect(TokenKind::LBrace)?;
                let mut args = vec![];
                if !self.at(&TokenKind::RBrace) {
                    loop {
                        args.push(self.parse_expr()?);
                        if self.at(&TokenKind::Comma) {
                            self.bump();
                            if self.at(&TokenKind::RBrace) {
                                break;
                            }
                            continue;
                        }
                        break;
                    }
                }
                let end = self.expect(TokenKind::RBrace)?;
                Ok(Expr::Call {
                    callee: Box::new(Expr::Ident("setOf".into(), start)),
                    args,
                    span: start.merge(end.span),
                })
            }
            TokenKind::For => self.parse_for_as_expr(),
            _ => Err(self.error(format!("unexpected token in expression: {:?}", self.cur.kind))),
        }
    }

    /// `k to v` call used by `[k : v]` map sugar.
    fn map_pair_to(k: Expr, v: Expr) -> Expr {
        let span = k.span().merge(v.span());
        Expr::Call {
            callee: Box::new(Expr::Ident("to".into(), span)),
            args: vec![k, v],
            span,
        }
    }

    fn parse_interp_parts(
        &mut self,
        parts: Vec<StringPart>,
        span: Span,
    ) -> Result<Expr, ParseError> {
        let mut out = Vec::new();
        for part in parts {
            match part {
                StringPart::Lit(s) => out.push(InterpPart::Lit(s)),
                StringPart::Ident(name) => {
                    out.push(InterpPart::Expr(Expr::Ident(name, span)));
                }
                StringPart::ExprSrc(src) => {
                    let trimmed = src.trim();
                    if trimmed.is_empty() {
                        return Err(ParseError {
                            message: "empty interpolation `${}`".into(),
                            span,
                        });
                    }
                    let e = parse_expr_str(trimmed).map_err(|e| ParseError {
                        message: format!("interpolation expression: {}", e.message),
                        span,
                    })?;
                    out.push(InterpPart::Expr(e));
                }
            }
        }
        if out.len() == 1 {
            if let InterpPart::Lit(s) = &out[0] {
                return Ok(Expr::String(s.clone(), span));
            }
        }
        Ok(Expr::Interp { parts: out, span })
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let start = self.bump().span;
        // `if a { ... }` — `{` belongs to then-branch, not a trailing closure on cond.
        let saved = self.allow_trailing_closure;
        self.allow_trailing_closure = false;
        let cond = self.parse_expr()?;
        self.allow_trailing_closure = saved;
        let then_branch = self.parse_block_expr()?;
        let else_branch = if self.at(&TokenKind::Else) {
            self.bump();
            if self.at(&TokenKind::If) {
                Some(Box::new(self.parse_if()?))
            } else {
                Some(Box::new(self.parse_block_expr()?))
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map(|e| e.span())
            .unwrap_or_else(|| then_branch.span());
        Ok(Expr::If {
            cond: Box::new(cond),
            then_branch: Box::new(then_branch),
            else_branch,
            span: start.merge(end),
        })
    }

    fn parse_for_as_expr(&mut self) -> Result<Expr, ParseError> {
        // for is a statement; wrap as block stmt expression returning Unit
        let start = self.cur.span;
        let stmt = self.parse_for_stmt()?;
        Ok(Expr::Block {
            stmts: vec![stmt],
            tail: None,
            span: start.merge(self.cur.span),
        })
    }

    fn parse_lambda_or_block(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(TokenKind::LBrace)?.span;
        self.parse_block_after_lbrace(start)
    }

    fn parse_block_expr(&mut self) -> Result<Expr, ParseError> {
        if self.at(&TokenKind::LBrace) {
            self.parse_lambda_or_block()
        } else {
            Err(self.error("expected `{` block"))
        }
    }

    fn parse_block_after_lbrace(&mut self, start: Span) -> Result<Expr, ParseError> {
        // Check lambda header using temporary collection of first tokens
        let checkpoint = self.checkpoint();
        let is_lambda = self.try_parse_lambda_params().is_ok();
        self.restore(checkpoint);

        if is_lambda {
            let params = self.try_parse_lambda_params()?;
            self.expect(TokenKind::Arrow)?;
            let (stmts, tail) = self.parse_block_contents()?;
            let end = self.expect(TokenKind::RBrace)?;
            let span = start.merge(end.span);
            return Ok(Expr::Lambda {
                params,
                body: Box::new(Expr::Block {
                    stmts,
                    tail,
                    span,
                }),
                span,
            });
        }

        let (stmts, tail) = self.parse_block_contents()?;
        let end = self.expect(TokenKind::RBrace)?;
        let span = start.merge(end.span);
        let uses_it = tail.as_ref().is_some_and(|e| expr_uses_ident(e, "it"));
        if stmts.is_empty() && uses_it {
            Ok(Expr::Lambda {
                params: vec!["it".into()],
                body: Box::new(Expr::Block {
                    stmts: vec![],
                    tail,
                    span,
                }),
                span,
            })
        } else {
            Ok(Expr::Block {
                stmts,
                tail,
                span,
            })
        }
    }

    fn try_parse_lambda_params(&mut self) -> Result<Vec<String>, ParseError> {
        if self.at(&TokenKind::Arrow) {
            return Ok(vec![]);
        }
        let mut params = vec![];
        let (p, _) = self.expect_ident()?;
        params.push(p);
        while self.at(&TokenKind::Comma) {
            self.bump();
            let (p, _) = self.expect_ident()?;
            params.push(p);
        }
        if self.at(&TokenKind::Arrow) {
            Ok(params)
        } else {
            Err(self.error("not lambda params"))
        }
    }

    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            cur: self.cur.clone(),
            // We need lexer position — extend Lexer
            pos: self.lexer.pos(),
        }
    }

    fn restore(&mut self, cp: Checkpoint) {
        self.cur = cp.cur;
        self.lexer.set_pos(cp.pos);
    }

    fn parse_block_contents(&mut self) -> Result<(Vec<Stmt>, Option<Box<Expr>>), ParseError> {
        let mut stmts = vec![];
        let mut tail = None;
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::Val) {
                let start = self.bump().span;
                let (name, _) = self.expect_ident()?;
                self.expect(TokenKind::Eq)?;
                let expr = self.parse_expr()?;
                stmts.push(Stmt::Val {
                    name,
                    span: start.merge(expr.span()),
                    expr,
                });
            } else if self.at(&TokenKind::Var) {
                let start = self.bump().span;
                let (name, _) = self.expect_ident()?;
                self.expect(TokenKind::Eq)?;
                let expr = self.parse_expr()?;
                stmts.push(Stmt::Var {
                    name,
                    span: start.merge(expr.span()),
                    expr,
                });
            } else if self.at(&TokenKind::For) {
                stmts.push(self.parse_for_stmt()?);
            } else if self.at(&TokenKind::Break) {
                let s = self.bump().span;
                stmts.push(Stmt::Break(s));
            } else if self.at(&TokenKind::Continue) {
                let s = self.bump().span;
                stmts.push(Stmt::Continue(s));
            } else if matches!(self.peek(), TokenKind::Ident(_)) {
                // Could be assign `name = expr` or expression
                let cp = self.checkpoint();
                let (name, nspan) = self.expect_ident()?;
                if self.at(&TokenKind::Eq) {
                    self.bump();
                    let expr = self.parse_expr()?;
                    stmts.push(Stmt::Assign {
                        name,
                        span: nspan.merge(expr.span()),
                        expr,
                    });
                } else {
                    self.restore(cp);
                    let expr = self.parse_expr()?;
                    // If next is `}` this is tail; else stmt
                    if self.at(&TokenKind::RBrace) {
                        tail = Some(Box::new(expr));
                        break;
                    } else {
                        stmts.push(Stmt::Expr(expr));
                    }
                }
            } else {
                let expr = self.parse_expr()?;
                if self.at(&TokenKind::RBrace) {
                    tail = Some(Box::new(expr));
                    break;
                } else {
                    stmts.push(Stmt::Expr(expr));
                }
            }
        }
        Ok((stmts, tail))
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
        let start = self.bump().span; // for
        // for x in xs { }  |  for (k, v) in m { }  |  for cond { }
        let saved = self.allow_trailing_closure;
        self.allow_trailing_closure = false;
        let result = (|| {
            if self.at(&TokenKind::LParen) {
                let cp = self.checkpoint();
                self.bump();
                if matches!(self.peek(), TokenKind::Ident(_)) {
                    let (k, _) = self.expect_ident()?;
                    if self.at(&TokenKind::Comma) {
                        self.bump();
                        if matches!(self.peek(), TokenKind::Ident(_)) {
                            let (v, _) = self.expect_ident()?;
                            if self.at(&TokenKind::RParen) {
                                self.bump();
                                if self.at(&TokenKind::In) {
                                    self.bump();
                                    let iter = self.parse_expr()?;
                                    self.allow_trailing_closure = saved;
                                    let body = self.parse_block_expr()?;
                                    return Ok(Stmt::ForIn {
                                        binding: ForBinding::Pair(k, v),
                                        iter,
                                        body: body.clone(),
                                        span: start.merge(body.span()),
                                    });
                                }
                            }
                        }
                    }
                }
                self.restore(cp);
            }
            if matches!(self.peek(), TokenKind::Ident(_)) {
                let cp = self.checkpoint();
                let (binding, _) = self.expect_ident()?;
                if self.at(&TokenKind::In) {
                    self.bump();
                    let iter = self.parse_expr()?;
                    self.allow_trailing_closure = saved;
                    let body = self.parse_block_expr()?;
                    return Ok(Stmt::ForIn {
                        binding: ForBinding::Name(binding),
                        iter,
                        body: body.clone(),
                        span: start.merge(body.span()),
                    });
                }
                self.restore(cp);
            }
            let cond = self.parse_expr()?;
            self.allow_trailing_closure = saved;
            let body = self.parse_block_expr()?;
            Ok(Stmt::ForCond {
                cond,
                body: body.clone(),
                span: start.merge(body.span()),
            })
        })();
        self.allow_trailing_closure = saved;
        result
    }
}

struct Checkpoint {
    cur: Token,
    pos: usize,
}

impl Pattern {
    fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(s) | Pattern::Int(_, s) | Pattern::Ident(_, s) => *s,
            Pattern::Variant { span, .. }
            | Pattern::Struct { span, .. }
            | Pattern::Tuple { span, .. }
            | Pattern::List { span, .. }
            | Pattern::Or(_, span) => *span,
        }
    }
}

fn expr_uses_ident(expr: &Expr, name: &str) -> bool {
    match expr {
        Expr::Ident(n, _) => n == name,
        Expr::Block { stmts, tail, .. } => {
            stmts.iter().any(|s| stmt_uses_ident(s, name))
                || tail.as_ref().is_some_and(|e| expr_uses_ident(e, name))
        }
        Expr::Lambda { body, .. } => expr_uses_ident(body, name),
        Expr::Call { callee, args, .. } => {
            expr_uses_ident(callee, name) || args.iter().any(|a| expr_uses_ident(a, name))
        }
        Expr::Binary { left, right, .. } | Expr::Pipeline { left, right, .. } => {
            expr_uses_ident(left, name) || expr_uses_ident(right, name)
        }
        Expr::Unary { expr, .. } | Expr::Field { base: expr, .. } => expr_uses_ident(expr, name),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            expr_uses_ident(cond, name)
                || expr_uses_ident(then_branch, name)
                || else_branch
                    .as_ref()
                    .is_some_and(|e| expr_uses_ident(e, name))
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr_uses_ident(scrutinee, name)
                || arms.iter().any(|a| {
                    expr_uses_ident(&a.body, name)
                        || a.guard.as_ref().is_some_and(|g| expr_uses_ident(g, name))
                })
        }
        Expr::MatchCond { arms, .. } => arms.iter().any(|a| {
            a.cond
                .as_ref()
                .is_some_and(|c| expr_uses_ident(c, name))
                || expr_uses_ident(&a.body, name)
        }),
        Expr::ListLit { elems, .. } => elems.iter().any(|e| expr_uses_ident(e, name)),
        Expr::StructLit { fields, .. } => fields.iter().any(|(_, e)| expr_uses_ident(e, name)),
        Expr::With { base, fields, .. } => {
            expr_uses_ident(base, name) || fields.iter().any(|(_, e)| expr_uses_ident(e, name))
        }
        Expr::TupleLit { elems, .. } => elems.iter().any(|e| expr_uses_ident(e, name)),
        Expr::Interp { parts, .. } => parts.iter().any(|p| match p {
            crate::InterpPart::Lit(_) => false,
            crate::InterpPart::Expr(e) => expr_uses_ident(e, name),
        }),
        Expr::Int(..) | Expr::Float(..) | Expr::Bool(..) | Expr::String(..) | Expr::Char(..) => false,
    }
}

fn stmt_uses_ident(stmt: &Stmt, name: &str) -> bool {
    match stmt {
        Stmt::Val { expr, .. } | Stmt::Var { expr, .. } | Stmt::Assign { expr, .. } => {
            expr_uses_ident(expr, name)
        }
        Stmt::Expr(e) => expr_uses_ident(e, name),
        Stmt::ForIn { iter, body, .. } => {
            expr_uses_ident(iter, name) || expr_uses_ident(body, name)
        }
        Stmt::ForCond { cond, body, .. } => {
            expr_uses_ident(cond, name) || expr_uses_ident(body, name)
        }
        Stmt::Break(_) | Stmt::Continue(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_for_in_ident_body_starts_with_assign() {
        // Regression: `for w in words { counts = ... }` must not parse
        // `words { counts = ... }` as a struct literal.
        let src = r#"
module T
val main = {
    var counts = 0
    val words = listOf(1)
    for w in words {
        counts = w
    }
    counts
}
"#;
        parse_module(src).expect("parse for-in with assign body");
    }

    #[test]
    fn parse_hello() {
        let src = r#"
module Hello
import std.io.{println}
val main = {
    println(42)
}
"#;
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "Hello");
        assert_eq!(m.items.len(), 1);
    }

    #[test]
    fn parse_match_bare_expr_arms() {
        let src = r#"
module M
val f = { n ->
    n match {
        0 -> 0
        1 -> 1
        x if x > 10 -> x - 1
        _ -> { n * 2 }
    }
}
"#;
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "M");
        let Item::Val(v) = &m.items[0] else {
            panic!("expected val");
        };
        let Expr::Lambda { body, .. } = &v.body else {
            panic!("expected lambda");
        };
        let Expr::Block { tail, .. } = body.as_ref() else {
            panic!("expected block body");
        };
        let Expr::Match { arms, .. } = tail.as_deref().expect("match tail") else {
            panic!("expected match");
        };
        assert_eq!(arms.len(), 4);
        assert!(!matches!(arms[0].body, Expr::Block { .. }));
        assert!(!matches!(arms[1].body, Expr::Block { .. }));
        assert!(!matches!(arms[2].body, Expr::Block { .. }));
        assert!(matches!(arms[3].body, Expr::Block { .. }));
    }


    #[test]
    fn parse_map_set_literal_sugars() {
        let m = parse_module(
            r#"
module M
val main = {
    val a = [:]
    val b = [1 : 10, 2 : 20]
    val c = #{}
    val d = #{1, 2, 3}
    a
}
"#,
        )
        .expect("parse map/set sugars");
        let Item::Val(v) = &m.items[0] else {
            panic!("expected val");
        };
        let Expr::Block { stmts, .. } = &v.body else {
            panic!("expected block");
        };
        assert_eq!(stmts.len(), 4);
        // [:] / [k:v] / #{} / #{…} desugar to mapOf/setOf calls
        for s in stmts {
            let Stmt::Val { expr, .. } = s else {
                panic!("expected val stmt");
            };
            assert!(
                matches!(expr, Expr::Call { .. }),
                "expected call sugar, got {expr:?}"
            );
        }
    }

    #[test]
    fn parse_list_patterns_variants() {
        parse_module("module M\nval f = { xs -> xs match { [] -> 0 _ -> 1 }\n}\n").unwrap();
        parse_module("module M\nval f = { xs -> xs match { [h] -> h _ -> 0 }\n}\n").unwrap();
        parse_module("module M\nval f = { xs -> xs match { [..rest] -> 0 _ -> 1 }\n}\n").unwrap();
        parse_module("module M\nval f = { xs -> xs match { [h, ..rest] -> h _ -> 0 }\n}\n").expect("h, ..rest");
    }

    #[test]
    fn parse_string_interpolation() {
        let src = r#"
module M
val main = {
    val name = "Lumia"
    val n = 1
    val s = "hi ${name} $n"
    s
}
"#;
        let m = parse_module(src).expect("parse");
        assert_eq!(m.name, "M");
    }
}

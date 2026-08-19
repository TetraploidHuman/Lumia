use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
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
            TokenKind::Float(n) => {
                let n = *n;
                let s = self.bump().span;
                Ok(Pattern::Float(n, s))
            }
            TokenKind::Minus => {
                // Negative numeric constant pattern: `-42` / `-1.5`.
                let minus = self.bump();
                match &self.cur.kind {
                    TokenKind::Int(n) => {
                        let n = *n;
                        let end = self.bump().span;
                        let neg = n.checked_neg().ok_or_else(|| {
                            self.error(
                                "integer pattern `-9223372036854775808` is out of range for Int (i64)",
                            )
                        })?;
                        Ok(Pattern::Int(neg, minus.span.merge(end)))
                    }
                    TokenKind::Float(n) => {
                        let n = *n;
                        let end = self.bump().span;
                        Ok(Pattern::Float(-n, minus.span.merge(end)))
                    }
                    _ => Err(self.error("expected number after `-` in pattern")),
                }
            }
            TokenKind::True => {
                let s = self.bump().span;
                Ok(Pattern::Bool(true, s))
            }
            TokenKind::False => {
                let s = self.bump().span;
                Ok(Pattern::Bool(false, s))
            }
            TokenKind::Char(c) => {
                let c = *c;
                let s = self.bump().span;
                Ok(Pattern::Char(c, s))
            }
            TokenKind::String(t) => {
                let t = t.clone();
                let s = self.bump().span;
                Ok(Pattern::String(self.intern.intern(&t), s))
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
            TokenKind::Ident => {
                let s = self.bump().span;
                let name = self.intern_span(s);
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
    pub(super) fn parse_struct_pattern_fields(
        &mut self,
    ) -> Result<Vec<(Sym, Pattern)>, ParseError> {
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
}

impl Pattern {
    pub(super) fn span(&self) -> Span {
        match self {
            Pattern::Wildcard(s)
            | Pattern::Int(_, s)
            | Pattern::Float(_, s)
            | Pattern::Bool(_, s)
            | Pattern::Char(_, s)
            | Pattern::String(_, s)
            | Pattern::Ident(_, s) => *s,
            Pattern::Variant { span, .. }
            | Pattern::Struct { span, .. }
            | Pattern::Tuple { span, .. }
            | Pattern::List { span, .. }
            | Pattern::Or(_, span) => *span,
        }
    }
}

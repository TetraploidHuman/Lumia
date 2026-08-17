use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_field_inits(&mut self) -> Result<Vec<(String, Expr)>, ParseError> {
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

    pub(super) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.parse_pipeline()
    }

    pub(super) fn parse_pipeline(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_or()?;
        while self.at(&TokenKind::GtGt) {
            let _ = self.bump();
            let right = self.parse_or()?;
            let span = left.span().merge(right.span());
            left = Expr::Pipeline {
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        // infix match / alt — same line only (newline `match {` is subjectless).
        loop {
            if self.at(&TokenKind::Match)
                && !self.newline_between(left.span().end, self.cur.span.start)
            {
                left = self.parse_match_suffix(left)?;
            } else if self.at(&TokenKind::Alt)
                && !self.newline_between(left.span().end, self.cur.span.start)
            {
                left = self.parse_alt_suffix(left)?;
            } else {
                break;
            }
        }
        Ok(left)
    }

    pub(super) fn parse_alt_suffix(&mut self, scrutinee: Expr) -> Result<Expr, ParseError> {
        self.bump(); // alt
        let alt = if self.at(&TokenKind::LBrace) {
            self.parse_block_expr()?
        } else {
            self.parse_expr()?
        };
        let span = scrutinee.span().merge(alt.span());
        Ok(Expr::Alt {
            scrutinee: Box::new(scrutinee),
            alt: Box::new(alt),
            span,
        })
    }

    pub(super) fn parse_match_suffix(&mut self, scrutinee: Expr) -> Result<Expr, ParseError> {
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
    pub(super) fn parse_match_cond(&mut self) -> Result<Expr, ParseError> {
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

    pub(super) fn parse_match_cond_arm(&mut self) -> Result<MatchCondArm, ParseError> {
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

    pub(super) fn parse_match_arm(&mut self) -> Result<MatchArm, ParseError> {
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
    pub(super) fn parse_or(&mut self) -> Result<Expr, ParseError> {
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

    pub(super) fn parse_and(&mut self) -> Result<Expr, ParseError> {
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

    pub(super) fn parse_cmp(&mut self) -> Result<Expr, ParseError> {
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

    /// Kotlin-style ranges (DESIGN §4.11): `a..b` → `rangeInclusive`; `a..<b` → `range`.
    pub(super) fn parse_range(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        let inclusive = if self.at(&TokenKind::DotDotEq) {
            self.bump();
            return Err(self.error(
                "`..=` was removed; use `a..b` for inclusive or `a..<b` for half-open (Kotlin-style)",
            ));
        } else if self.at(&TokenKind::DotDot) {
            true
        } else if self.at(&TokenKind::DotDotLt) {
            false
        } else {
            return Ok(left);
        };
        self.bump();
        let right = self.parse_add()?;
        let span = left.span().merge(right.span());
        let name = if inclusive { "rangeInclusive" } else { "range" };
        Ok(Expr::Call {
            callee: Box::new(Expr::Ident(name.into(), span)),
            args: vec![left, right],
            span,
        })
    }

    pub(super) fn parse_add(&mut self) -> Result<Expr, ParseError> {
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
    pub(super) fn parse_to(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        while self.at(&TokenKind::To) {
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

    pub(super) fn parse_mul(&mut self) -> Result<Expr, ParseError> {
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

    pub(super) fn parse_unary(&mut self) -> Result<Expr, ParseError> {
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

    pub(super) fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.at(&TokenKind::LParen)
                && !self.newline_between(expr.span().end, self.cur.span.start)
            {
                // call — same-line only so `x\n(2, y) ->` is the next match arm,
                // not a call `x(2, y)`.
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
                let span = expr
                    .span()
                    .merge(args.last().map(|a| a.span()).unwrap_or(end.span));
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

    pub(super) fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match &self.cur.kind {
            TokenKind::Error(msg) => {
                let msg = msg.clone();
                let s = self.bump().span;
                Err(ParseError {
                    message: msg,
                    span: s,
                })
            }
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
            // Hard keyword that still denotes the `to` pair constructor as a primary.
            TokenKind::To => {
                let s = self.bump().span;
                Ok(Expr::Ident("to".into(), s))
            }
            TokenKind::If => self.parse_if(),
            TokenKind::Match => self.parse_match_cond(),
            TokenKind::Return => {
                let start = self.bump().span;
                let value = self.parse_expr()?;
                let span = start.merge(value.span());
                Ok(Expr::Return {
                    value: Box::new(value),
                    span,
                })
            }
            // `effect { … }` — visual effect region; same as a block (DESIGN §2.2.1).
            TokenKind::Effect => {
                self.bump();
                self.parse_lambda_or_block()
            }
            // `spawn { … }` — task body (DESIGN §11.2).
            TokenKind::Spawn => {
                let start = self.bump().span;
                let body = self.parse_lambda_or_block()?;
                let span = start.merge(body.span());
                Ok(Expr::Spawn {
                    body: Box::new(body),
                    span,
                })
            }
            // `scope { … }` / `scope(sched) { … }` — structured concurrency (DESIGN §11.2).
            TokenKind::Scope => {
                let start = self.bump().span;
                let scheduler = if self.at(&TokenKind::LParen) {
                    self.bump();
                    let e = self.parse_expr()?;
                    self.expect(TokenKind::RParen)?;
                    Some(Box::new(e))
                } else {
                    None
                };
                let body = self.parse_lambda_or_block()?;
                let span = start.merge(body.span());
                Ok(Expr::Scope {
                    scheduler,
                    body: Box::new(body),
                    span,
                })
            }
            TokenKind::Trait | TokenKind::Instance | TokenKind::Requires => Err(self.error(
                "expected expression (`trait` / `instance` / `requires` are item-level only)",
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
            _ => Err(self.error(format!(
                "unexpected token in expression: {}",
                self.cur.kind
            ))),
        }
    }

    /// `k to v` call used by `[k : v]` map sugar.
    pub(super) fn map_pair_to(k: Expr, v: Expr) -> Expr {
        let span = k.span().merge(v.span());
        Expr::Call {
            callee: Box::new(Expr::Ident("to".into(), span)),
            args: vec![k, v],
            span,
        }
    }

    pub(super) fn parse_interp_parts(
        &mut self,
        parts: Vec<StringPart>,
        span: Span,
    ) -> Result<Expr, ParseError> {
        let mut out = Vec::new();
        for part in parts {
            match part {
                StringPart::Lit(s) => out.push(InterpPart::Lit(s)),
                StringPart::Ident { name, abs_start } => {
                    let end = abs_start + name.len() as u32;
                    out.push(InterpPart::Expr(Expr::Ident(
                        name,
                        Span::new(abs_start, end),
                    )));
                }
                StringPart::ExprSrc { src, abs_start } => {
                    let lead = src.len() - src.trim_start().len();
                    let trimmed = src.trim();
                    if trimmed.is_empty() {
                        return Err(ParseError {
                            message: "empty interpolation `${}`".into(),
                            span: Span::new(abs_start.saturating_sub(2), abs_start.saturating_add(1)),
                        });
                    }
                    let base = abs_start + lead as u32;
                    let mut e = parse_expr_str(trimmed).map_err(|e| ParseError {
                        message: format!("interpolation expression: {}", e.message),
                        span: e.span.shift(base),
                    })?;
                    crate::stamp::offset_expr(&mut e, base);
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

    pub(super) fn parse_if(&mut self) -> Result<Expr, ParseError> {
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
}

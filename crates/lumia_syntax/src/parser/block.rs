use super::util::expr_uses_ident;
use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_for_as_expr(&mut self) -> Result<Expr, ParseError> {
        // for is a statement; wrap as block stmt expression returning Unit
        let start = self.cur.span;
        let stmt = self.parse_for_stmt()?;
        Ok(Expr::Block {
            stmts: vec![stmt],
            tail: None,
            span: start.merge(self.cur.span),
        })
    }

    pub(super) fn parse_lambda_or_block(&mut self) -> Result<Expr, ParseError> {
        let start = self.expect(TokenKind::LBrace)?.span;
        self.parse_block_after_lbrace(start)
    }

    pub(super) fn parse_block_expr(&mut self) -> Result<Expr, ParseError> {
        if self.at(&TokenKind::LBrace) {
            self.parse_lambda_or_block()
        } else {
            Err(self.error("expected `{` block"))
        }
    }

    pub(super) fn parse_block_after_lbrace(&mut self, start: Span) -> Result<Expr, ParseError> {
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
                body: Box::new(Expr::Block { stmts, tail, span }),
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
            Ok(Expr::Block { stmts, tail, span })
        }
    }

    pub(super) fn try_parse_lambda_params(&mut self) -> Result<Vec<String>, ParseError> {
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
    pub(super) fn parse_block_contents(
        &mut self,
    ) -> Result<(Vec<Stmt>, Option<Box<Expr>>), ParseError> {
        let mut stmts = vec![];
        let mut tail = None;
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            if self.at(&TokenKind::Val) {
                let start = self.bump().span;
                let pat = self.parse_pattern()?;
                self.expect(TokenKind::Eq)?;
                let expr = self.parse_expr()?;
                stmts.push(Stmt::Val {
                    pat,
                    span: start.merge(expr.span()),
                    expr,
                });
            } else if self.at(&TokenKind::Var) {
                let start = self.bump().span;
                // `var` stays a single name (mutable slots are not patterns).
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

    pub(super) fn parse_for_stmt(&mut self) -> Result<Stmt, ParseError> {
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

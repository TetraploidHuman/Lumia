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
        // Single speculative parse: keep params on success, restore once on miss.
        let checkpoint = self.checkpoint();
        match self.try_parse_lambda_params() {
            Ok((params, param_tys)) => {
                self.expect(TokenKind::Arrow)?;
                // Lambda bodies: stop before a column-0 top-level item so a missing
                // `}` cannot swallow the next `val`/`type`/… declaration.
                let (stmts, tail) = self.parse_block_contents(true)?;
                let end = self.expect_rbrace_or_recover()?;
                let span = start.merge(end);
                return Ok(Expr::Lambda {
                    params,
                    param_tys,
                    bare_it: false,
                    body: Box::new(Expr::Block { stmts, tail, span }),
                    span,
                });
            }
            Err(_) => self.restore(checkpoint),
        }

        let (stmts, tail) = self.parse_block_contents(false)?;
        let end = self.expect_rbrace_or_recover()?;
        let span = start.merge(end);
        let uses_it = tail.as_ref().is_some_and(|e| expr_uses_ident(e, "it"));
        if stmts.is_empty() && uses_it {
            Ok(Expr::Lambda {
                params: vec![self.intern_word("it")],
                param_tys: vec![None],
                bare_it: true,
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

    /// Like `expect(RBrace)`, but if a top-level item starter is next, report the
    /// missing `}` without consuming it (item-level recovery resumes there).
    fn expect_rbrace_or_recover(&mut self) -> Result<Span, ParseError> {
        if self.at(&TokenKind::RBrace) {
            return Ok(self.bump().span);
        }
        Err(self.error(format!(
            "expected {}, found {}",
            TokenKind::RBrace,
            self.cur.kind
        )))
    }

    pub(super) fn try_parse_lambda_params(
        &mut self,
    ) -> Result<(Vec<Sym>, Vec<Option<String>>), ParseError> {
        if self.at(&TokenKind::Arrow) {
            return Ok((vec![], vec![]));
        }
        let mut params = vec![];
        let mut param_tys = vec![];
        let (p, ty) = self.parse_annotated_binder()?;
        params.push(p);
        param_tys.push(ty);
        while self.at(&TokenKind::Comma) {
            self.bump();
            let (p, ty) = self.parse_annotated_binder()?;
            params.push(p);
            param_tys.push(ty);
        }
        if self.at(&TokenKind::Arrow) {
            Ok((params, param_tys))
        } else {
            Err(self.error("not lambda params"))
        }
    }
    pub(super) fn parse_block_contents(
        &mut self,
        stop_at_column0_item: bool,
    ) -> Result<(Vec<Stmt>, Option<Box<Expr>>), ParseError> {
        let mut stmts = vec![];
        let mut tail = None;
        while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
            // Only for `{ params -> … }` bodies: a column-0 item starter ends the
            // lambda early. Plain `{ … }` blocks still allow unindented local `val`.
            if stop_at_column0_item && self.at_column0_item_start() {
                break;
            }
            if self.at(&TokenKind::Val) {
                let start = self.bump().span;
                let pat = self.parse_pattern()?;
                let ty = if matches!(pat, Pattern::Ident(_, _)) {
                    self.parse_optional_type_ann()?
                } else if self.at(&TokenKind::Colon) {
                    return Err(
                        self.error("type ascription is only allowed on simple `val` binders")
                    );
                } else {
                    None
                };
                self.expect(TokenKind::Eq)?;
                let expr = match self.parse_expr() {
                    Ok(expr) => expr,
                    Err(e) => {
                        let span = e.span;
                        self.errors.push(e);
                        // Consume until a new statement/expression boundary.
                        self.synchronize_block_stmt(stop_at_column0_item);
                        self.hole_expr(span)
                    }
                };
                stmts.push(Stmt::Val {
                    pat,
                    ty,
                    span: start.merge(expr.span()),
                    expr,
                });
            } else if self.at(&TokenKind::Var) {
                let start = self.bump().span;
                // `var` stays a single name (mutable slots are not patterns).
                let (name, _) = self.expect_ident()?;
                let ty = self.parse_optional_type_ann()?;
                self.expect(TokenKind::Eq)?;
                let expr = match self.parse_expr() {
                    Ok(expr) => expr,
                    Err(e) => {
                        let span = e.span;
                        self.errors.push(e);
                        self.synchronize_block_stmt(stop_at_column0_item);
                        self.hole_expr(span)
                    }
                };
                stmts.push(Stmt::Var {
                    name,
                    ty,
                    span: start.merge(expr.span()),
                    expr,
                });
            } else if self.at(&TokenKind::For) {
                let stmt = match self.parse_for_stmt() {
                    Ok(s) => s,
                    Err(e) => {
                        let span = e.span;
                        self.errors.push(e);
                        self.synchronize_block_stmt(stop_at_column0_item);
                        Stmt::Expr(self.hole_expr(span))
                    }
                };
                stmts.push(stmt);
            } else if self.at(&TokenKind::Break) {
                let s = self.bump().span;
                stmts.push(Stmt::Break(s));
            } else if self.at(&TokenKind::Continue) {
                let s = self.bump().span;
                stmts.push(Stmt::Continue(s));
            } else if self.at_ident() {
                // Could be assign `name = expr` or expression
                let cp = self.checkpoint();
                let (name, nspan) = self.expect_ident()?;
                if self.at(&TokenKind::Eq) {
                    self.bump();
                    let expr = match self.parse_expr() {
                        Ok(expr) => expr,
                        Err(e) => {
                            let span = e.span;
                            self.errors.push(e);
                            self.synchronize_block_stmt(stop_at_column0_item);
                            self.hole_expr(span)
                        }
                    };
                    stmts.push(Stmt::Assign {
                        name,
                        span: nspan.merge(expr.span()),
                        expr,
                    });
                } else {
                    self.restore(cp);
                    let expr = match self.parse_expr() {
                        Ok(expr) => expr,
                        Err(e) => {
                            let span = e.span;
                            self.errors.push(e);
                            self.synchronize_block_stmt(stop_at_column0_item);
                            self.hole_expr(span)
                        }
                    };
                    // If next is `}` this is tail; else stmt
                    if self.at(&TokenKind::RBrace) {
                        tail = Some(Box::new(expr));
                        break;
                    } else {
                        stmts.push(Stmt::Expr(expr));
                    }
                }
            } else {
                let expr = match self.parse_expr() {
                    Ok(expr) => expr,
                    Err(e) => {
                        let span = e.span;
                        self.errors.push(e);
                        self.synchronize_block_stmt(stop_at_column0_item);
                        self.hole_expr(span)
                    }
                };
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
                if self.at_ident() {
                    let (k, _) = self.expect_ident()?;
                    if self.at(&TokenKind::Comma) {
                        self.bump();
                        if self.at_ident() {
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
            if self.at_ident() {
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

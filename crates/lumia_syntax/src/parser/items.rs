use super::*;

impl<'a> Parser<'a> {
    pub(super) fn parse_module(&mut self) -> Result<Module, ParseError> {
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

    pub(super) fn parse_import(&mut self) -> Result<Import, ParseError> {
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
                let alias = if self.at(&TokenKind::As) {
                    self.bump();
                    let (a, _) = self.expect_ident()?;
                    Some(a)
                } else {
                    None
                };
                ns.push(ImportedName { name: n, alias });
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
            // `import a.b as alias` — DESIGN §9.3.
            let alias = if self.at(&TokenKind::As) {
                self.bump();
                let (a, _) = self.expect_ident()?;
                Some(a)
            } else {
                None
            };
            let last = path.pop().unwrap();
            ImportNames::Single(ImportedName { name: last, alias })
        };

        Ok(Import {
            path,
            names,
            span: start.merge(self.cur.span),
        })
    }

    pub(super) fn parse_item(&mut self) -> Result<Item, ParseError> {
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
        } else if self.at(&TokenKind::Trait) {
            if is_priv {
                return Err(self.error("`priv trait` is not supported"));
            }
            self.parse_trait_item()
        } else if self.at(&TokenKind::Instance) {
            if is_priv {
                return Err(self.error("`priv instance` is not supported"));
            }
            self.parse_instance_item()
        } else if self.at(&TokenKind::Requires) {
            Err(self.error("`requires` is only valid after a trait name"))
        } else {
            Err(self.error("expected `val`, `type`, `foreign`, `trait`, or `instance` item"))
        }
    }

    /// `trait Name [requires A, B] { val m = … }` (DESIGN §3.6).
    pub(super) fn parse_trait_item(&mut self) -> Result<Item, ParseError> {
        let start = self.bump().span; // trait
        let (name, _) = self.expect_ident()?;
        let mut requires = vec![];
        if self.at(&TokenKind::Requires) {
            self.bump();
            loop {
                let (r, _) = self.expect_ident()?;
                requires.push(r);
                if self.at(&TokenKind::Comma) {
                    self.bump();
                    continue;
                }
                break;
            }
        }
        self.expect(TokenKind::LBrace)?;
        let methods = self.parse_trait_methods()?;
        let end = self.expect(TokenKind::RBrace)?;
        Ok(Item::Trait(crate::TraitItem {
            name,
            requires,
            methods,
            span: start.merge(end.span),
        }))
    }

    /// `instance Trait for Type { val m = … }`
    pub(super) fn parse_instance_item(&mut self) -> Result<Item, ParseError> {
        let start = self.bump().span; // instance
        let (trait_name, _) = self.expect_ident()?;
        if !self.at(&TokenKind::For) {
            return Err(self.error("expected `for` after trait name in instance"));
        }
        self.bump(); // for
        let (type_name, _) = self.expect_ident()?;
        self.expect(TokenKind::LBrace)?;
        let methods = self.parse_trait_methods()?;
        let end = self.expect(TokenKind::RBrace)?;
        Ok(Item::Instance(crate::InstanceItem {
            trait_name,
            type_name,
            methods,
            span: start.merge(end.span),
        }))
    }

    pub(super) fn parse_trait_methods(&mut self) -> Result<Vec<ValItem>, ParseError> {
        let mut methods = vec![];
        while self.at(&TokenKind::Val) {
            methods.push(self.parse_val_item()?);
        }
        if !self.at(&TokenKind::RBrace) {
            return Err(self.error("expected `val` method or `}` in trait/instance body"));
        }
        Ok(methods)
    }

    /// `foreign "C" [pure] fn name(x: Int, y: Int) -> Int`
    pub(super) fn parse_foreign_item(&mut self) -> Result<Item, ParseError> {
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

    pub(super) fn parse_val_item(&mut self) -> Result<ValItem, ParseError> {
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

    pub(super) fn parse_type_item(&mut self) -> Result<TypeItem, ParseError> {
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
}

//! Prelude collection constructors (`listOf` / `setOf` / `mapOf`).
//!
//! These are **not** [`lumia_hir::Builtin`] variants: they are surface names in
//! [`lumia_hir::PRELUDE_CTORS`], inferred here, and lowered to Core `AllocList` /
//! `AllocSet` / `AllocMap` (see `lumia_core::lower`). Keep arity/effect handling
//! in sync with that comment on `PRELUDE_CTORS`.

use super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::Expr;

impl Infer {
    /// Returns `Some` when `name` is a prelude collection ctor.
    pub(crate) fn try_infer_prelude_ctor(
        &mut self,
        name: &str,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<Option<(Type, Effect)>, TypeError> {
        match name {
            "listOf" => Ok(Some(self.infer_list_of(args, span)?)),
            "setOf" => Ok(Some(self.infer_set_of(args, span)?)),
            "mapOf" => Ok(Some(self.infer_map_of(args, span)?)),
            _ => Ok(None),
        }
    }

    fn infer_list_of(
        &mut self,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let mut aes = Effect::pure();
        let elem = self.fresh();
        for a in args {
            let (t, e) = self.infer_expr(a)?;
            aes = self.union_eff(aes, e);
            self.unify_at(span, elem.clone(), t)?;
        }
        Ok((Type::List(Box::new(self.prune(elem))), aes))
    }

    fn infer_set_of(
        &mut self,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let mut aes = Effect::pure();
        let elem = self.fresh();
        for a in args {
            let (t, e) = self.infer_expr(a)?;
            aes = self.union_eff(aes, e);
            self.unify_at(span, elem.clone(), t)?;
        }
        Ok((Type::Set(Box::new(self.prune(elem))), aes))
    }

    fn infer_map_of(
        &mut self,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let mut aes = Effect::pure();
        let k = self.fresh();
        let v = self.fresh();
        if !args.len().is_multiple_of(2) {
            return Err(at(
                span,
                "mapOf expects an even number of key/value arguments",
            ));
        }
        for chunk in args.chunks(2) {
            let (kt, ke) = self.infer_expr(&chunk[0])?;
            let (vt, ve) = self.infer_expr(&chunk[1])?;
            aes = self.union3_eff(aes, ke, ve);
            self.unify_at(span, k.clone(), kt)?;
            self.unify_at(span, v.clone(), vt)?;
        }
        Ok((
            Type::Map(Box::new(self.prune(k)), Box::new(self.prune(v))),
            aes,
        ))
    }
}

//! Call expression inference (including UFCS dispatch).

use super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::Expr;

impl Infer {
    pub(crate) fn infer_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        // Special-case listOf(...): List[T] with unified element type
        if let Expr::Var(name, _) = callee {
            if name == "listOf" {
                let mut aes = Effect::pure();
                let elem = self.fresh();
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    aes = self.union_eff(aes, e);
                    self.unify_at(span, elem.clone(), t)?;
                }
                return Ok((Type::List(Box::new(self.prune(elem))), aes));
            }
            if name == "setOf" {
                let mut aes = Effect::pure();
                let elem = self.fresh();
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    aes = self.union_eff(aes, e);
                    self.unify_at(span, elem.clone(), t)?;
                }
                return Ok((Type::Set(Box::new(self.prune(elem))), aes));
            }
            if name == "mapOf" {
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
                return Ok((
                    Type::Map(Box::new(self.prune(k)), Box::new(self.prune(v))),
                    aes,
                ));
            }
            // UFCS trait method: unbound `method(recv, …)` → mangled instance fun.
            // Free top-level `method` wins when bound (checked below via lookup).
            if self.lookup(name).is_none() && !args.is_empty() {
                if let Some(result) = self.try_infer_trait_ufcs(name, args, span)? {
                    return Ok(result);
                }
            }
        }
        let (ct, ce) = self.infer_expr(callee)?;
        let mut aes = Effect::pure();
        let mut ats = vec![];
        for a in args {
            let (t, e) = self.infer_expr(a)?;
            ats.push(t);
            aes = self.union_eff(aes, e);
        }
        let ret = self.fresh();
        // Open effect when callee is not yet a concrete Fun — allows HOFs to
        // pick up IO from effectful callbacks (Var stays open vs Pure; Io binds).
        let call_eff = match self.prune(ct.clone()) {
            Type::Fun(_, _, e) => e,
            _ => self.fresh_eff(),
        };
        self.unify_at(span, ct, Type::Fun(ats, Box::new(ret.clone()), call_eff))?;
        let fun_eff = self.prune_eff(call_eff);
        Ok((self.prune(ret), self.union3_eff(ce, aes, fun_eff)))
    }
}

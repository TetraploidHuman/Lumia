//! Call expression inference (including UFCS dispatch).

use super::Infer;
use crate::types::{Effect, Type, TypeError};
use lumi_hir::Expr;

impl Infer {
    pub(crate) fn infer_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: lumi_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        if let Expr::Var(name, _) = callee {
            // Prelude ctors (`listOf`/`setOf`/`mapOf`) — see `prelude_ctors`.
            if let Some(result) = self.try_infer_prelude_ctor(name, args, span)? {
                return Ok(result);
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

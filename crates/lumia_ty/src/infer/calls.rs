//! Call expression inference (including UFCS dispatch).

use super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_call(
        &mut self,
        callee: &Expr,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        if let Expr::Var(name, _) = callee {
            // Prelude ctors (`listOf`/`setOf`/`mapOf`) — see `prelude_ctors`.
            if let Some(result) = self.try_infer_prelude_ctor(name, args, span)? {
                return Ok(result);
            }
            // Overload `join`: Task.join() vs List.join(sep) — pick by receiver.
            if name == "join" && self.lookup(name).is_none() {
                return self.infer_join_surface(args, span);
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

    /// Surface `join` / `.join(…)`: arity alone cannot distinguish Task vs List.
    fn infer_join_surface(
        &mut self,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let io = Effect::io();
        match args.len() {
            1 => {
                let (tt, te) = self.infer_expr(&args[0])?;
                match self.prune(tt.clone()) {
                    Type::List(_) => Err(at(
                        span,
                        "List.join requires a separator: xs.join(sep)",
                    )),
                    Type::Task(_) | Type::Var(_) => {
                        let elem = self.fresh();
                        self.unify_at(span, tt, Type::Task(Box::new(elem.clone())))?;
                        crate::span_facts::insert_unique_span_fact(
                            &mut self.traits.join_rewrites,
                            span,
                            Builtin::TaskJoin,
                            "join",
                        )?;
                        Ok((elem, self.union_eff(io, te)))
                    }
                    other => Err(at(
                        span,
                        format!(
                            "join: expected Task[T], got {}",
                            crate::display::display_type(&other, &[])
                        ),
                    )),
                }
            }
            2 => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (st, se) = self.infer_expr(&args[1])?;
                self.unify_at(span, st, Type::String)?;
                match self.prune(lt.clone()) {
                    Type::Task(_) => {
                        return Err(at(
                            span,
                            "Task.join takes no separator (use t.join())",
                        ));
                    }
                    Type::List(t) => {
                        self.unify_at(span, *t, Type::String)?;
                    }
                    Type::Var(_) => {
                        self.unify_at(span, lt, Type::List(Box::new(Type::String)))?;
                    }
                    other => {
                        return Err(at(
                            span,
                            format!(
                                "join: expected List[String], got {}",
                                crate::display::display_type(&other, &[])
                            ),
                        ));
                    }
                }
                crate::span_facts::insert_unique_span_fact(
                    &mut self.traits.join_rewrites,
                    span,
                    Builtin::ListJoin,
                    "join",
                )?;
                Ok((Type::String, self.union_eff(le, se)))
            }
            n => Err(at(
                span,
                format!("join: expected 1 or 2 arguments, got {n}"),
            )),
        }
    }
}

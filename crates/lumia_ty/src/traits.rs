//! Trait / instance / UFCS resolution helpers.

use crate::infer::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Expr, Item, Module};
use rustc_hash::FxHashMap as HashMap;

impl Infer {
    pub(crate) fn is_ord(&self, t: &Type) -> bool {
        match t {
            Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Var(_) => true,
            Type::Adt { name, .. } => self.ord_instances.contains(name),
            _ => false,
        }
    }

    pub(crate) fn mark_num(&mut self, t: &Type) {
        if let Type::Var(v) = self.prune(t.clone()) {
            self.num_vars.insert(v);
        }
    }

    /// Resolve unbound UFCS `method(recv, …)` via `trait_methods`.
    /// Concrete ADT → rewrite to mangled; open Var → record trait predicate (mono later).
    pub(crate) fn try_infer_trait_ufcs(
        &mut self,
        method: &str,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<Option<(Type, Effect)>, TypeError> {
        let mut aes = Effect::pure();
        let (recv_ty, re) = self.infer_expr(&args[0])?;
        aes = self.union_eff(aes, re);
        let mut ats = vec![recv_ty.clone()];
        for a in &args[1..] {
            let (t, e) = self.infer_expr(a)?;
            ats.push(t);
            aes = self.union_eff(aes, e);
        }
        match self.prune(recv_ty.clone()) {
            Type::Adt { name: ty_name, .. } => {
                let cands = self
                    .trait_methods
                    .get(&(ty_name.clone(), method.to_string()))
                    .cloned()
                    .unwrap_or_default();
                match cands.as_slice() {
                    [] => Ok(None),
                    [mangled] => {
                        let ct = self.lookup(mangled).ok_or_else(|| {
                            at(
                                span,
                                format!("trait method `{method}` for `{ty_name}` is not in scope"),
                            )
                        })?;
                        let ret = self.fresh();
                        let call_eff = match self.prune(ct.clone()) {
                            Type::Fun(_, _, e) => e,
                            _ => self.fresh_eff(),
                        };
                        self.unify_at(span, ct, Type::Fun(ats, Box::new(ret.clone()), call_eff))?;
                        self.ufcs_rewrites.insert(span, mangled.clone());
                        let fun_eff = self.prune_eff(call_eff);
                        Ok(Some((self.prune(ret), self.union_eff(aes, fun_eff))))
                    }
                    many => {
                        let names: Vec<_> = many
                            .iter()
                            .filter_map(|m| m.strip_prefix("__").and_then(|s| s.split('_').next()))
                            .collect();
                        Err(at(
                            span,
                            format!(
                                "ambiguous trait method `{method}` for `{ty_name}` \
                                 (candidates: {}); qualify or rename",
                                names.join(", ")
                            ),
                        ))
                    }
                }
            }
            Type::Var(v) => {
                let Some(trait_name) = self.method_trait.get(method).cloned() else {
                    return Ok(None);
                };
                // Peek a sample impl for arity/effect only — do NOT unify it with the
                // open call (that froze `{ x -> x.toInt() }` to the first instance type).
                let sample = self
                    .trait_methods
                    .values()
                    .flatten()
                    .find(|m| m.ends_with(&format!("_{method}")))
                    .cloned();
                let ret = self.fresh();
                let fun_eff = if let Some(sample) = sample {
                    let ct = self.lookup(&sample).ok_or_else(|| {
                        at(span, format!("trait method `{method}` is not in scope"))
                    })?;
                    match self.prune(ct) {
                        Type::Fun(params, _, e) => {
                            if params.len() != ats.len() {
                                return Err(at(
                                    span,
                                    format!(
                                        "trait method `{method}` expects {} args, got {}",
                                        params.len(),
                                        ats.len()
                                    ),
                                ));
                            }
                            self.prune_eff(e)
                        }
                        _ => Effect::pure(),
                    }
                } else {
                    // Trait declared but no instance yet — open ret; call site checks.
                    Effect::pure()
                };
                self.trait_vars
                    .entry(v)
                    .or_default()
                    .push((trait_name, method.to_string()));
                // Leave HIR as `method(recv,…)` — Core mono resolves after specialize.
                Ok(Some((ret, self.union_eff(aes, fun_eff))))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn check_num_bind(&mut self, v: u32, t: &Type) -> Result<(), TypeError> {
        if !self.num_vars.contains(&v) {
            return Ok(());
        }
        match self.prune(t.clone()) {
            Type::Int | Type::Float | Type::Var(_) => Ok(()),
            other => Err(TypeError::Message(format!(
                "numeric type required for arithmetic, got {other}"
            ))),
        }
    }

    pub(crate) fn check_trait_bind(&mut self, v: u32, t: &Type) -> Result<(), TypeError> {
        let Some(preds) = self.trait_vars.get(&v).cloned() else {
            return Ok(());
        };
        match self.prune(t.clone()) {
            Type::Var(u) => {
                let entry = self.trait_vars.entry(u).or_default();
                for p in preds {
                    if !entry.contains(&p) {
                        entry.push(p);
                    }
                }
                Ok(())
            }
            Type::Adt { name, .. } => {
                for (tr, method) in preds {
                    if !self.instances.contains(&(tr.clone(), name.clone())) {
                        return Err(TypeError::Message(format!(
                            "no `instance {tr} for {name}` (required by `.{method}()`)"
                        )));
                    }
                }
                Ok(())
            }
            other => Err(TypeError::Message(format!(
                "trait method requires an ADT with an instance, got {other}"
            ))),
        }
    }
}

/// Rewrite UFCS `method(recv,…)` callees to mangled `__Trait_Type_method`.
pub(crate) fn apply_ufcs_rewrites(
    module: &mut Module,
    rewrites: &HashMap<lumia_syntax::Span, String>,
) {
    for item in &mut module.items {
        match item {
            Item::Fun(f) => rewrite_ufcs_in_expr(&mut f.body, rewrites),
            Item::Val { body, .. } => rewrite_ufcs_in_expr(body, rewrites),
        }
    }
}

pub(crate) fn rewrite_ufcs_in_expr(
    expr: &mut Expr,
    rewrites: &HashMap<lumia_syntax::Span, String>,
) {
    match expr {
        Expr::Call { callee, args, span } => {
            for a in args.iter_mut() {
                rewrite_ufcs_in_expr(a, rewrites);
            }
            if let Some(mangled) = rewrites.get(span) {
                **callee = Expr::Var(mangled.clone(), *span);
            } else {
                rewrite_ufcs_in_expr(callee, rewrites);
            }
        }
        Expr::BuiltinCall { args, .. } | Expr::AdtNew { args, .. } => {
            for a in args {
                rewrite_ufcs_in_expr(a, rewrites);
            }
        }
        Expr::Let { value, body, .. } => {
            rewrite_ufcs_in_expr(value, rewrites);
            rewrite_ufcs_in_expr(body, rewrites);
        }
        Expr::Assign { value, .. } | Expr::Unary { expr: value, .. } => {
            rewrite_ufcs_in_expr(value, rewrites);
        }
        Expr::Lambda { body, .. } => rewrite_ufcs_in_expr(body, rewrites),
        Expr::Binary { left, right, .. } => {
            rewrite_ufcs_in_expr(left, rewrites);
            rewrite_ufcs_in_expr(right, rewrites);
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            rewrite_ufcs_in_expr(cond, rewrites);
            rewrite_ufcs_in_expr(then_branch, rewrites);
            rewrite_ufcs_in_expr(else_branch, rewrites);
        }
        Expr::Loop {
            cond, body, step, ..
        } => {
            rewrite_ufcs_in_expr(cond, rewrites);
            rewrite_ufcs_in_expr(body, rewrites);
            if let Some(s) = step {
                rewrite_ufcs_in_expr(s, rewrites);
            }
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                rewrite_ufcs_in_expr(s, rewrites);
            }
        }
        Expr::Return { value, .. } => rewrite_ufcs_in_expr(value, rewrites),
        Expr::Alt { scrutinee, alt, .. } => {
            rewrite_ufcs_in_expr(scrutinee, rewrites);
            rewrite_ufcs_in_expr(alt, rewrites);
        }
        Expr::Var(_, _)
        | Expr::Int(_, _)
        | Expr::Float(_, _)
        | Expr::Bool(_, _)
        | Expr::String(_, _)
        | Expr::Char(_, _)
        | Expr::Unit(_)
        | Expr::Break(_)
        | Expr::Continue(_) => {}
    }
}

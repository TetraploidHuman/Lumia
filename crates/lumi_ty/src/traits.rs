//! Trait / instance / UFCS resolution helpers.

use crate::infer::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumi_hir::{Expr, Item, Module};
use rustc_hash::FxHashMap as HashMap;

impl Infer {
    pub(crate) fn is_ord(&self, t: &Type) -> bool {
        match t {
            Type::Int | Type::Float | Type::Bool | Type::String | Type::Char => true,
            // Open vars are provisionally Ord; `mark_ord` + `check_ord_bind` reject
            // List/Map/Fun/… when the var is later grounded.
            Type::Var(_) => true,
            Type::Adt { name, .. } => self.traits.ord_instances.contains(name),
            _ => false,
        }
    }

    /// DESIGN: `==` is structural; functions have no structural Eq (and the
    /// language does not expose reference equality). Containers/ADTs are Eq
    /// only when every element/field type is Eq.
    pub(crate) fn is_eq(&mut self, t: &Type) -> bool {
        match self.prune(t.clone()) {
            Type::Fun(_, _, _) => false,
            Type::Var(_) => true,
            Type::List(e) | Type::Set(e) => self.is_eq(&e),
            Type::Map(k, v) => {
                let ek = self.is_eq(&k);
                let ev = self.is_eq(&v);
                ek && ev
            }
            Type::Adt { params, .. } => {
                for p in params {
                    if !self.is_eq(&p) {
                        return false;
                    }
                }
                true
            }
            Type::Tuple(ts) | Type::TuplePrefix(ts) => {
                for p in ts {
                    if !self.is_eq(&p) {
                        return false;
                    }
                }
                true
            }
            Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Unit => true,
        }
    }

    pub(crate) fn type_mentions_fun(t: &Type) -> bool {
        match t {
            Type::Fun(_, _, _) => true,
            Type::List(e) | Type::Set(e) => Self::type_mentions_fun(e),
            Type::Map(k, v) => Self::type_mentions_fun(k) || Self::type_mentions_fun(v),
            Type::Adt { params, .. } => params.iter().any(Self::type_mentions_fun),
            Type::Tuple(ts) | Type::TuplePrefix(ts) => ts.iter().any(Self::type_mentions_fun),
            Type::Var(_)
            | Type::Int
            | Type::Float
            | Type::Bool
            | Type::String
            | Type::Char
            | Type::Unit => false,
        }
    }

    pub(crate) fn mark_num(&mut self, t: &Type) {
        if let Type::Var(v) = self.prune(t.clone()) {
            self.uni.num_vars.insert(v);
        }
    }

    pub(crate) fn mark_ord(&mut self, t: &Type) {
        if let Type::Var(v) = self.prune(t.clone()) {
            self.uni.ord_vars.insert(v);
        }
    }

    pub(crate) fn mark_eq(&mut self, t: &Type) {
        if let Type::Var(v) = self.prune(t.clone()) {
            self.uni.eq_vars.insert(v);
        }
    }

    /// Resolve unbound UFCS `method(recv, …)` via `trait_methods`.
    /// Concrete ADT → rewrite to mangled; open Var → record trait predicate (mono later).
    pub(crate) fn try_infer_trait_ufcs(
        &mut self,
        method: &str,
        args: &[Expr],
        span: lumi_syntax::Span,
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
        match self.prune(recv_ty) {
            Type::Adt { name: ty_name, .. } => {
                let key = (ty_name, method.to_string());
                let cands = self
                    .traits
                    .trait_methods
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                match cands.as_slice() {
                    [] => Ok(None),
                    [mangled] => {
                        let ct = self.lookup(mangled).ok_or_else(|| {
                            at(
                                span,
                                format!("trait method `{method}` for `{}` is not in scope", key.0),
                            )
                        })?;
                        let ret = self.fresh();
                        let call_eff = match self.prune(ct.clone()) {
                            Type::Fun(_, _, e) => e,
                            _ => self.fresh_eff(),
                        };
                        self.unify_at(span, ct, Type::Fun(ats, Box::new(ret.clone()), call_eff))?;
                        self.traits.ufcs_rewrites.insert(span, mangled.clone());
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
                                "ambiguous trait method `{method}` for `{}` \
                                 (candidates: {}); qualify or rename",
                                key.0,
                                names.join(", ")
                            ),
                        ))
                    }
                }
            }
            Type::Var(v) => {
                let Some(trait_name) = self.traits.method_trait.get(method).cloned() else {
                    return Ok(None);
                };
                // Peek a sample impl for arity/effect only — do NOT unify it with the
                // open call (that froze `{ x -> x.toInt() }` to the first instance type).
                let sample = self
                    .traits
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
                self.traits
                    .trait_vars
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
        if !self.uni.num_vars.contains(&v) {
            return Ok(());
        }
        match self.prune(t.clone()) {
            Type::Int | Type::Float | Type::Var(_) => Ok(()),
            other => Err(TypeError::Message(format!(
                "numeric type required for arithmetic, got {other}"
            ))),
        }
    }

    pub(crate) fn check_ord_bind(&mut self, v: u32, t: &Type) -> Result<(), TypeError> {
        if !self.uni.ord_vars.contains(&v) {
            return Ok(());
        }
        match self.prune(t.clone()) {
            Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Var(_) => {
                Ok(())
            }
            Type::Adt { name, .. } if self.traits.ord_instances.contains(&name) => Ok(()),
            other => Err(TypeError::Message(format!(
                "`<`/`<=`/`>`/`>=` need Ord (scalars or `instance Ord for T`), got {other}"
            ))),
        }
    }

    pub(crate) fn check_eq_bind(&mut self, v: u32, t: &Type) -> Result<(), TypeError> {
        if !self.uni.eq_vars.contains(&v) {
            return Ok(());
        }
        let t = self.prune(t.clone());
        if matches!(t, Type::Fun(_, _, _)) || Self::type_mentions_fun(&t) {
            return Err(TypeError::Message(
                "`==`/`!=` need structural Eq; functions are not comparable".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn check_trait_bind(&mut self, v: u32, t: &Type) -> Result<(), TypeError> {
        let Some(preds) = self.traits.trait_vars.get(&v).cloned() else {
            return Ok(());
        };
        match self.prune(t.clone()) {
            Type::Var(u) => {
                let entry = self.traits.trait_vars.entry(u).or_default();
                for p in preds {
                    if !entry.contains(&p) {
                        entry.push(p);
                    }
                }
                Ok(())
            }
            Type::Adt { name, .. } => {
                for (tr, method) in preds {
                    if !self.traits.instances.contains(&(tr.clone(), name.clone())) {
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
    rewrites: &HashMap<lumi_syntax::Span, String>,
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
    rewrites: &HashMap<lumi_syntax::Span, String>,
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
        Expr::With { base, fields, .. } => {
            rewrite_ufcs_in_expr(base, rewrites);
            for (_, e) in fields {
                rewrite_ufcs_in_expr(e, rewrites);
            }
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

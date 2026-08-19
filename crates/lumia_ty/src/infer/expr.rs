//! Expression inference.

use super::Infer;
use crate::types::{at, expr_span, Effect, Type, TypeError};
use lumia_hir::Expr;
use lumia_syntax::{BinOp, UnOp};
use std::sync::Arc;

impl Infer {
    pub(crate) fn infer_expr(&mut self, expr: &Expr) -> Result<(Type, Effect), TypeError> {
        let (t, e) = self.infer_expr_inner(expr)?;
        // `Let` has no own span; `expr_span` falls through to the value. Pushing
        // the body's type there would clobber the value's entry (e.g. `channel(1)`
        // becoming `Unit` after `val ch = channel(1); …`). Value and body already
        // record their own spans when inferred.
        if !matches!(expr, Expr::Let { .. }) {
            self.type_at.push((expr_span(expr), t.clone()));
        }
        Ok((t, e))
    }

    pub(crate) fn infer_expr_inner(&mut self, expr: &Expr) -> Result<(Type, Effect), TypeError> {
        match expr {
            Expr::Int(_, _) => Ok((Type::Int, Effect::pure())),
            Expr::Float(_, _) => Ok((Type::Float, Effect::pure())),
            Expr::Bool(_, _) => Ok((Type::Bool, Effect::pure())),
            Expr::String(_, _) => Ok((Type::String, Effect::pure())),
            Expr::Char(_, _) => Ok((Type::Char, Effect::pure())),
            Expr::Unit(_) => Ok((Type::Unit, Effect::pure())),
            Expr::Var(name, span) => {
                let t = self
                    .lookup(name.as_str())
                    .ok_or_else(|| at(*span, format!("unbound variable `{name}`")))?;
                self.check_name_visible(name.as_str(), *span)?;
                Ok((t, Effect::pure()))
            }
            Expr::Let {
                name,
                value,
                body,
                mutable,
                ty,
            } => self.infer_let(name.as_str(), value, body, *mutable, ty.as_deref()),
            Expr::Assign { name, value, span } => self.infer_assign(name.as_str(), value, *span),
            Expr::Lambda {
                params,
                param_ann,
                body,
                span,
            } => self.infer_lambda(params, param_ann, body, *span),
            Expr::Call { callee, args, span } => self.infer_call(callee, args, *span),
            Expr::BuiltinCall { name, args, span } => self.infer_builtin_call(name, args, *span),
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => self.infer_binary(*op, left, right, *span),
            Expr::Unary { op, expr, span } => self.infer_unary(*op, expr, *span),
            Expr::If {
                cond,
                then_branch,
                else_branch,
                span,
            } => self.infer_if(cond, then_branch, else_branch, *span),
            Expr::Loop {
                cond,
                body,
                step,
                span,
            } => self.infer_loop(cond, body, step.as_deref(), *span),
            Expr::Break(span) => self.infer_break_continue("break", *span),
            Expr::Continue(span) => self.infer_break_continue("continue", *span),
            Expr::Return { value, span } => self.infer_return(value, *span),
            Expr::Alt {
                scrutinee,
                alt,
                span,
            } => self.infer_alt(scrutinee, alt, *span),
            Expr::With { base, fields, span } => self.infer_with(base, fields, *span),
            Expr::AdtNew {
                adt_name,
                variant,
                args,
                ..
            } => self.infer_adt_new(adt_name.as_str(), variant.as_str(), args),
            Expr::Seq { stmts, .. } => self.infer_seq(stmts),
        }
    }

    fn infer_with(
        &mut self,
        base: &Expr,
        fields: &[(lumia_syntax::Sym, Expr)],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let mut seen_fields = rustc_hash::FxHashSet::default();
        for (fname, _) in fields {
            if !seen_fields.insert(fname.clone()) {
                return Err(at(
                    span,
                    format!("duplicate field `{fname}` in product `with`"),
                ));
            }
        }
        let (base_ty, mut eff) = self.infer_expr(base)?;
        let base_ty = self.prune(base_ty);
        let (name, params) = match base_ty {
            Type::Adt { name, params } => (name, params),
            Type::Var(_) => {
                // Open receiver: fields must uniquely identify a product, then
                // constrain the var (e.g. `{ p -> p with { x = 10 } }` with only
                // `Point` in scope). Never override a *concrete* other product —
                // that case is handled by the Adt arm above.
                let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                let Some(name) = self.unique_product_for_fields(&names) else {
                    return Err(at(
                        span,
                        "product `with` on an open receiver needs fields that uniquely \
                         identify one product type",
                    ));
                };
                let order =
                    self.products.products.get(&name).cloned().ok_or_else(|| {
                        at(span, format!("unknown product type `{name}` in `with`"))
                    })?;
                let params: Vec<Type> = (0..order.len()).map(|_| self.fresh()).collect();
                self.unify_at(
                    span,
                    base_ty,
                    Type::Adt {
                        name: name.clone(),
                        params: params.clone(),
                    },
                )?;
                (name, params)
            }
            _ => {
                return Err(at(
                    span,
                    "product `with` requires a concrete product-typed base",
                ));
            }
        };
        let order = self
            .products
            .products
            .get(&name)
            .cloned()
            .ok_or_else(|| at(span, format!("unknown product type `{name}` in `with`")))?;
        let mut by_name: rustc_hash::FxHashMap<lumia_syntax::Sym, Type> =
            rustc_hash::FxHashMap::default();
        for (fname, e) in fields {
            if !order.iter().any(|f| f == fname.as_str()) {
                return Err(at(
                    span,
                    format!("unknown field `{fname}` in `{name}` `with`"),
                ));
            }
            let (t, e_eff) = self.infer_expr(e)?;
            eff = self.union_eff(eff, e_eff);
            by_name.insert(fname.clone(), t);
        }
        let mut out_params = Vec::with_capacity(order.len());
        for (i, f) in order.iter().enumerate() {
            if let Some(t) = by_name.remove(f) {
                if let Some(old) = params.get(i) {
                    self.unify_at(span, t.clone(), old.clone())?;
                }
                out_params.push(t);
            } else if let Some(old) = params.get(i) {
                out_params.push(old.clone());
            } else {
                out_params.push(self.fresh());
            }
        }
        crate::span_facts::insert_unique_span_fact(
            &mut self.ctrl.with_rewrites,
            span,
            name.clone(),
            "with",
        )?;
        Ok((
            Type::Adt {
                name,
                params: out_params,
            },
            eff,
        ))
    }

    /// Product whose field set is the unique owner of every name in `fields`.
    fn unique_product_for_fields(&self, fields: &[&str]) -> Option<lumia_syntax::Sym> {
        let mut names = fields.iter().copied();
        let first = names.next()?;
        let mut set: rustc_hash::FxHashSet<lumia_syntax::Sym> = self
            .products
            .products
            .iter()
            .filter(|(_, fs)| fs.iter().any(|f| f == first))
            .map(|(n, _)| n.clone())
            .collect();
        if set.is_empty() {
            return None;
        }
        for f in names {
            set.retain(|prod| {
                self.products
                    .products
                    .get(prod)
                    .is_some_and(|fs| fs.iter().any(|x| x == f))
            });
            if set.is_empty() {
                return None;
            }
        }
        if set.len() == 1 {
            set.into_iter().next()
        } else {
            None
        }
    }

    fn infer_let(
        &mut self,
        name: &str,
        value: &Expr,
        body: &Expr,
        mutable: bool,
        ann: Option<&str>,
    ) -> Result<(Type, Effect), TypeError> {
        let (vt, ve) = self.infer_expr(value)?;
        let vt = if let Some(ann) = ann {
            let expect = self.resolve_type_ann(
                ann,
                expr_span(value),
                &format!("in type ascription for `{name}`"),
            )?;
            self.unify_at(expr_span(value), vt, expect.clone())?;
            expect
        } else {
            vt
        };
        self.push();
        // Immutable lets generalize (HM let-poly); `var` stays monomorphic.
        if mutable {
            self.bind_mut(name.into(), vt, true);
        } else {
            let scheme = self.generalize(vt);
            self.bind_scheme(name.into(), scheme, false);
        }
        let (bt, be) = self.infer_expr(body)?;
        self.pop();
        Ok((bt, self.union_eff(ve, be)))
    }

    fn infer_assign(
        &mut self,
        name: &str,
        value: &Expr,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let expect = self
            .lookup(name)
            .ok_or_else(|| at(span, format!("unbound `{name}` in assign")))?;
        if !self.is_mutable(name) {
            return Err(at(
                span,
                format!("cannot assign to immutable binding `{name}` (use `var`)"),
            ));
        }
        let (vt, ve) = self.infer_expr(value)?;
        // Widen Fun effects (Pure ⊔ Io = Io) and update the binding so
        // later calls see the lub — equality unify would reject or, with
        // the old Pure↔Io hole, silently keep Pure.
        let joined = self.join_types(expect, vt, span)?;
        self.rebind(name, joined)?;
        Ok((Type::Unit, ve))
    }

    fn infer_lambda(
        &mut self,
        params: &[lumia_syntax::Sym],
        param_ann: &[Option<String>],
        body: &Expr,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        self.push();
        let mut pts = vec![];
        for (i, p) in params.iter().enumerate() {
            let tv = if let Some(Some(ann)) = param_ann.get(i) {
                self.resolve_type_ann(ann, span, &format!("in type ascription for `{p}`"))?
            } else {
                self.fresh()
            };
            pts.push(tv.clone());
            self.bind(p.to_string(), tv);
        }
        let ret_tv = self.fresh();
        self.ctrl.return_stack.push(ret_tv.clone());
        // `break`/`continue` must not cross a lambda (same as other languages).
        let saved_loop = self.ctrl.loop_depth;
        self.ctrl.loop_depth = 0;
        let body_result = self.infer_expr(body);
        self.ctrl.loop_depth = saved_loop;
        let (rt, re) = body_result?;
        self.unify_at(span, rt, ret_tv.clone())?;
        self.ctrl.return_stack.pop();
        self.pop();
        Ok((Type::Fun(pts, Arc::new(ret_tv), re), Effect::pure()))
    }

    fn infer_break_continue(
        &self,
        kw: &str,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        if self.ctrl.loop_depth == 0 {
            return Err(at(span, format!("`{kw}` is only allowed inside a loop")));
        }
        Ok((Type::Unit, Effect::pure()))
    }

    fn infer_binary(
        &mut self,
        op: BinOp,
        left: &Expr,
        right: &Expr,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let (lt, le) = self.infer_expr(left)?;
        let (rt, re) = self.infer_expr(right)?;
        let eff = self.union_eff(le, re);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                let lt = self.prune(lt);
                let rt = self.prune(rt);
                // `instance Num for T` + same ADT: `+`/`*` via `__Num_T_{add,mul}`.
                if matches!(op, BinOp::Add | BinOp::Mul) {
                    if let (Type::Adt { name: a, .. }, Type::Adt { name: b, .. }) = (&lt, &rt) {
                        if a == b && self.traits.num_instances.contains(a) {
                            self.unify_at(span, lt.clone(), rt)?;
                            return Ok((self.prune(lt), eff));
                        }
                    }
                }
                match (&lt, &rt) {
                    (Type::Float, Type::Float) => Ok((Type::Float, eff)),
                    // Mixed Float/Int: codegen sitofp (polymorphic literals / Num MVP).
                    (Type::Float, Type::Int) | (Type::Int, Type::Float) => Ok((Type::Float, eff)),
                    (Type::Float, Type::Var(_)) => {
                        self.mark_num(&rt);
                        self.unify_at(span, rt, Type::Float)?;
                        Ok((Type::Float, eff))
                    }
                    (Type::Var(_), Type::Float) => {
                        self.mark_num(&lt);
                        self.unify_at(span, lt, Type::Float)?;
                        Ok((Type::Float, eff))
                    }
                    // Leave open for let-poly: `{ x -> x + x }` and `{ x -> x + 1 }`.
                    // `num_vars` blocks later unify with String/Bool/ADT.
                    (Type::Var(_), Type::Var(_)) => {
                        self.mark_num(&lt);
                        self.mark_num(&rt);
                        self.unify_at(span, lt.clone(), rt)?;
                        Ok((self.prune(lt), eff))
                    }
                    (Type::Var(_), Type::Int) | (Type::Int, Type::Var(_)) => {
                        self.mark_num(&lt);
                        self.mark_num(&rt);
                        let v = match (&lt, &rt) {
                            (Type::Var(_), _) => lt,
                            _ => rt,
                        };
                        Ok((v, eff))
                    }
                    _ => {
                        self.unify_at(span, lt, Type::Int)?;
                        self.unify_at(span, rt, Type::Int)?;
                        Ok((Type::Int, eff))
                    }
                }
            }
            BinOp::Eq | BinOp::Ne => {
                // DESIGN: structural Eq only — not function / reference equality.
                self.unify_at(span, lt.clone(), rt)?;
                let t = self.prune(lt);
                if self.is_eq(&t) {
                    self.mark_eq(&t);
                    Ok((Type::Bool, eff))
                } else {
                    Err(at(
                        span,
                        format!(
                            "`==`/`!=` need structural Eq; functions are not comparable, got {t}"
                        ),
                    ))
                }
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                // DESIGN Ord: scalars always; ADT/product when `instance Ord for T`.
                self.unify_at(span, lt.clone(), rt)?;
                let t = self.prune(lt);
                if self.is_ord(&t) {
                    self.mark_ord(&t);
                    Ok((Type::Bool, eff))
                } else {
                    Err(at(
                        span,
                        format!(
                            "`<`/`<=`/`>`/`>=` need Ord (scalars or `instance Ord for T`), got {t}"
                        ),
                    ))
                }
            }
            BinOp::And | BinOp::Or => Err(at(
                span,
                "`and`/`or` should have been desugared to `if` before typing",
            )),
        }
    }

    fn infer_unary(
        &mut self,
        op: UnOp,
        expr: &Expr,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let (t, e) = self.infer_expr(expr)?;
        match op {
            UnOp::Neg => {
                let t = self.prune(t);
                match t {
                    Type::Float => Ok((Type::Float, e)),
                    Type::Var(_) => {
                        self.mark_num(&t);
                        Ok((t, e))
                    }
                    _ => {
                        self.unify_at(span, t, Type::Int)?;
                        Ok((Type::Int, e))
                    }
                }
            }
            UnOp::Not => {
                self.unify_at(span, t, Type::Bool)?;
                Ok((Type::Bool, e))
            }
        }
    }

    fn infer_if(
        &mut self,
        cond: &Expr,
        then_branch: &Expr,
        else_branch: &Expr,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let (ct, ce) = self.infer_expr(cond)?;
        self.unify_at(span, ct, Type::Bool)?;
        let (tt, te) = self.infer_expr(then_branch)?;
        let (et, ee) = self.infer_expr(else_branch)?;
        let joined = self.join_types(tt, et, span)?;
        Ok((joined, self.union3_eff(ce, te, ee)))
    }

    fn infer_loop(
        &mut self,
        cond: &Expr,
        body: &Expr,
        step: Option<&Expr>,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        self.ctrl.loop_depth += 1;
        let result = (|| {
            let (ct, ce) = self.infer_expr(cond)?;
            self.unify_at(span, ct, Type::Bool)?;
            let (_, be) = self.infer_expr(body)?;
            let se = if let Some(s) = step {
                self.infer_expr(s)?.1
            } else {
                Effect::pure()
            };
            Ok((Type::Unit, self.union3_eff(ce, be, se)))
        })();
        self.ctrl.loop_depth -= 1;
        result
    }

    fn infer_return(
        &mut self,
        value: &Expr,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        let Some(expect) = self.ctrl.return_stack.last().cloned() else {
            return Err(at(
                span,
                "`return` is only allowed inside a function or closure",
            ));
        };
        let (vt, ve) = self.infer_expr(value)?;
        self.unify_at(span, vt, expect)?;
        // Diverges: fresh type unifies with any use-site expectation.
        Ok((self.fresh(), ve))
    }

    fn infer_alt(
        &mut self,
        scrutinee: &Expr,
        alt: &Expr,
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        use crate::alt::AltKind;
        self.ctrl.alt_scrutinee_depth += 1;
        let (st, se) = self.infer_expr(scrutinee)?;
        self.ctrl.alt_scrutinee_depth -= 1;
        let st = self.prune(st);
        match st {
            Type::Adt { name, params } if lumia_hir::is_option(&name) && params.len() == 1 => {
                crate::span_facts::insert_unique_span_fact(
                    &mut self.ctrl.alt_kinds,
                    span,
                    AltKind::Option,
                    "alt",
                )?;
                let payload = params[0].clone();
                let (rhs_ty, ae) = self.infer_expr(alt)?;
                let rhs_p = self.prune(rhs_ty.clone());
                // DESIGN §8.1: rhs is the success payload `T`, not another Option.
                // `None alt Some(x)` used to unify Option into an open payload Var,
                // so the desugar else-arm returned an ADT while the type said `T`
                // (Float → println IEEE bits; Int → accidental Show of Some).
                if matches!(&rhs_p, Type::Adt { name, .. } if lumia_hir::is_option(name)) {
                    return Err(at(
                        span,
                        format!(
                            "`alt` rhs must be the Option payload type, got {}",
                            self.zonk_type(rhs_p)
                        ),
                    ));
                }
                self.unify_at(span, rhs_ty, payload.clone())?;
                Ok((payload, self.union_eff(se, ae)))
            }
            Type::Adt { name, params } if lumia_hir::is_result(&name) && params.len() == 2 => {
                crate::span_facts::insert_unique_span_fact(
                    &mut self.ctrl.alt_kinds,
                    span,
                    AltKind::Result,
                    "alt",
                )?;
                let ok_ty = params[0].clone();
                let err_ty = params[1].clone();
                self.push();
                self.bind("err".into(), err_ty);
                let (rhs_ty, ae) = self.infer_expr(alt)?;
                self.pop();
                let rhs_p = self.prune(rhs_ty.clone());
                if matches!(&rhs_p, Type::Adt { name, .. } if lumia_hir::is_result(name)) {
                    return Err(at(
                        span,
                        format!(
                            "`alt` rhs must be the Result Ok payload type, got {}",
                            self.zonk_type(rhs_p)
                        ),
                    ));
                }
                self.unify_at(span, rhs_ty, ok_ty.clone())?;
                Ok((ok_ty, self.union_eff(se, ae)))
            }
            other => Err(at(
                span,
                format!(
                    "`alt` needs Option or Result, got {}",
                    self.zonk_type(other)
                ),
            )),
        }
    }

    fn infer_adt_new(
        &mut self,
        adt_name: &str,
        variant: &str,
        args: &[Expr],
    ) -> Result<(Type, Effect), TypeError> {
        let mut eff = Effect::pure();
        let mut arg_tys = vec![];
        for a in args {
            let (t, e) = self.infer_expr(a)?;
            arg_tys.push(t);
            eff = self.union_eff(eff, e);
        }
        if adt_name == "__Tuple" {
            return Ok((Type::Tuple(arg_tys), eff));
        }
        // Result[T, E]: Ok fills T (E fresh); Err fills E (T fresh).
        let params = if lumia_hir::is_result(adt_name) {
            match (variant, arg_tys.as_slice()) {
                ("Ok", [t]) => vec![t.clone(), self.fresh()],
                ("Err", [e]) => vec![self.fresh(), e.clone()],
                _ if arg_tys.is_empty() => vec![self.fresh(), self.fresh()],
                _ => arg_tys,
            }
        } else if self.products.sum_max_arity.contains_key(adt_name) {
            // User/prelude sums: parametric slots only; recursive spines are `Self`.
            let nparams = self.products.sum_max_arity[adt_name];
            let rec = self
                .products
                .sum_field_recursive
                .get(variant)
                .cloned()
                .unwrap_or_else(|| vec![false; arg_tys.len()]);
            let base = self
                .products
                .sum_ctors
                .get(variant)
                .map(|(_, _, off)| *off)
                .unwrap_or(0);
            let mut params: Vec<Type> = (0..nparams).map(|_| self.fresh()).collect();
            let self_ty = Type::Adt {
                name: adt_name.into(),
                params: params.clone(),
            };
            let mut pslot = base;
            for (i, t) in arg_tys.into_iter().enumerate() {
                if rec.get(i).copied().unwrap_or(false) {
                    self.unify(t, self_ty.clone())?;
                } else if pslot < params.len() {
                    self.unify(t, params[pslot].clone())?;
                    pslot += 1;
                }
            }
            params = params.into_iter().map(|p| self.prune(p)).collect();
            params
        } else {
            // Products: payload types are the params.
            arg_tys
        };
        Ok((
            Type::Adt {
                name: adt_name.into(),
                params,
            },
            eff,
        ))
    }

    fn infer_seq(&mut self, stmts: &[Expr]) -> Result<(Type, Effect), TypeError> {
        let mut eff = Effect::pure();
        let mut last = Type::Unit;
        for s in stmts {
            let (t, e) = self.infer_expr(s)?;
            last = t;
            eff = self.union_eff(eff, e);
        }
        Ok((last, eff))
    }
}

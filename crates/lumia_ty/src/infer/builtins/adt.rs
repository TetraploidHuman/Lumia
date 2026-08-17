//! BuiltinCall typing — adt family.

use super::super::unify::occurs;
use super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_adt_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::AdtTag => {
                let (_, e) = self.infer_expr(&args[0])?;
                Ok((Type::Int, e))
            }
            Builtin::AdtField => self.infer_adt_field(args, span),
            _ => unreachable!("non-adt builtin dispatched to infer_adt_builtin"),
        }
    }

    fn infer_adt_field(
        &mut self,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        // 2 args: tuple/positional `.0`; 3 args: product field with expected ADT name
        // — or unresolved field name when index is -1 (ambiguous at lower).
        if args.len() != 2 && args.len() != 3 {
            return Err(at(span, "adt_field takes 2 or 3 arguments"));
        }
        let (recv_ty, ae) = self.infer_expr(&args[0])?;
        let (it, ie) = self.infer_expr(&args[1])?;
        self.unify_at(span, it, Type::Int)?;
        let mut eff = self.union_eff(ae, ie);
        let third = if args.len() == 3 {
            let (nt, ne) = self.infer_expr(&args[2])?;
            self.unify_at(span, nt, Type::String)?;
            eff = self.union_eff(eff, ne);
            match &args[2] {
                Expr::String(s, _) => Some(s.as_str()),
                _ => None,
            }
        } else {
            None
        };
        // Ambiguous `.field`: index -1, 3rd arg = field name.
        if matches!(&args[1], Expr::Int(-1, _)) {
            let fname =
                third.ok_or_else(|| at(span, "unresolved field projection missing field name"))?;
            return self.infer_unresolved_product_field(recv_ty, fname, span, eff);
        }
        let expect_adt = third;
        let idx = match &args[1] {
            Expr::Int(n, _) if *n >= 0 => *n as usize,
            _ => {
                return Err(at(span, "adt_field index must be a non-negative literal"));
            }
        };
        let elem = match self.prune(recv_ty.clone()) {
            Type::Adt { name, params } => {
                self.field_from_adt(span, &name, &params, expect_adt, idx)?
            }
            Type::Tuple(ts) => self.field_from_tuple(span, &ts, expect_adt, idx)?,
            Type::TuplePrefix(ts) => {
                self.field_from_tuple_prefix(span, recv_ty, ts, expect_adt, idx)?
            }
            Type::Var(_) => self.constrain_open_product_receiver(span, recv_ty, expect_adt, idx)?,
            other => {
                return Err(at(
                    span,
                    format!("field projection: expected product/tuple, got {other:?}"),
                ));
            }
        };
        Ok((elem, eff))
    }

    fn infer_unresolved_product_field(
        &mut self,
        recv_ty: Type,
        field: &str,
        span: lumia_syntax::Span,
        eff: Effect,
    ) -> Result<(Type, Effect), TypeError> {
        match self.prune(recv_ty) {
            Type::Adt { name, params } => {
                let order = self.products.products.get(&name).ok_or_else(|| {
                    at(
                        span,
                        format!("unknown product type `{name}` for field `{field}`"),
                    )
                })?;
                let idx = order
                    .iter()
                    .position(|f| f == field)
                    .ok_or_else(|| at(span, format!("type `{name}` has no field `{field}`")))?;
                let elem = params.get(idx).cloned().ok_or_else(|| {
                    at(
                        span,
                        format!("field `{field}` index {idx} out of range for `{name}`"),
                    )
                })?;
                crate::span_facts::insert_unique_span_fact(
                    &mut self.ctrl.product_field_rewrites,
                    span,
                    (name, idx as i64),
                    "product field",
                )?;
                Ok((elem, eff))
            }
            Type::Var(_) => Err(at(
                span,
                format!(
                    "cannot resolve field `{field}` on an open type \
                     (ambiguous across product types; give the receiver a concrete product type)"
                ),
            )),
            other => Err(at(
                span,
                format!("field `{field}`: expected product type, got {other:?}"),
            )),
        }
    }

    fn field_from_adt(
        &mut self,
        span: lumia_syntax::Span,
        name: &str,
        params: &[Type],
        expect_adt: Option<&str>,
        idx: usize,
    ) -> Result<Type, TypeError> {
        if let Some(want) = expect_adt {
            // Variant patterns pass ctor name (`Ok`/`Err`/`Some`);
            // product patterns / field proj pass the ADT name.
            if lumia_hir::is_result(name) && (want == "Ok" || want == "Err") {
                if idx != 0 {
                    return Err(at(
                        span,
                        format!("Result::{want} has a single payload (index 0), got {idx}"),
                    ));
                }
                let pi = if want == "Ok" { 0 } else { 1 };
                return params.get(pi).cloned().ok_or_else(|| {
                    at(
                        span,
                        format!("Result::{want} payload missing (params {})", params.len()),
                    )
                });
            }
            if lumia_hir::is_option(name) && want == "Some" {
                if idx != 0 {
                    return Err(at(
                        span,
                        format!("Option::Some has a single payload (index 0), got {idx}"),
                    ));
                }
                return params
                    .first()
                    .cloned()
                    .ok_or_else(|| at(span, "Option::Some payload missing"));
            }
            if name != want {
                // Sum variant patterns pass the ctor name (`Circle`), not the ADT.
                if let Some((adt, arity, offset)) = self.products.sum_ctors.get(want) {
                    if adt != name {
                        return Err(at(
                            span,
                            format!("field projection expects type `{adt}`, got `{name}`"),
                        ));
                    }
                    if idx >= *arity {
                        return Err(at(
                            span,
                            format!(
                                "variant `{want}` has {arity} field(s); index {idx} out of range"
                            ),
                        ));
                    }
                    // Recursive spines are the ADT itself; parametric fields use
                    // concatenated slots (skipping recursive indices).
                    let rec = self
                        .products
                        .sum_field_recursive
                        .get(want)
                        .and_then(|v| v.get(idx).copied())
                        .unwrap_or(false);
                    if rec {
                        return Ok(Type::Adt {
                            name: name.into(),
                            params: params.to_vec(),
                        });
                    }
                    let local = (0..idx)
                        .filter(|&i| {
                            !self
                                .products
                                .sum_field_recursive
                                .get(want)
                                .and_then(|v| v.get(i).copied())
                                .unwrap_or(false)
                        })
                        .count();
                    return params.get(offset + local).cloned().ok_or_else(|| {
                        at(
                            span,
                            format!(
                                "field index {idx} out of range for `{name}` (arity {})",
                                params.len()
                            ),
                        )
                    });
                } else {
                    return Err(at(
                        span,
                        format!("field projection expects type `{want}`, got `{name}`"),
                    ));
                }
            }
        }
        params.get(idx).cloned().ok_or_else(|| {
            at(
                span,
                format!(
                    "field index {idx} out of range for `{name}` (arity {})",
                    params.len()
                ),
            )
        })
    }

    fn field_from_tuple(
        &self,
        span: lumia_syntax::Span,
        ts: &[Type],
        expect_adt: Option<&str>,
        idx: usize,
    ) -> Result<Type, TypeError> {
        if expect_adt.is_some() {
            return Err(at(span, "named product field applied to a tuple"));
        }
        ts.get(idx).cloned().ok_or_else(|| {
            at(
                span,
                format!("tuple index {idx} out of range (arity {})", ts.len()),
            )
        })
    }

    fn field_from_tuple_prefix(
        &mut self,
        span: lumia_syntax::Span,
        recv_ty: Type,
        ts: Vec<Type>,
        expect_adt: Option<&str>,
        idx: usize,
    ) -> Result<Type, TypeError> {
        if expect_adt.is_some() {
            return Err(at(span, "named product field applied to a tuple"));
        }
        if idx < ts.len() {
            return Ok(ts[idx].clone());
        }
        // Extend prefix to cover this index.
        let mut prefix = ts;
        while prefix.len() <= idx {
            prefix.push(self.fresh());
        }
        let field_ty = prefix[idx].clone();
        let extended = Type::TuplePrefix(prefix);
        // `unify(Var→Prefix[α], Prefix[α,β])` prunes the var to the short prefix
        // first, then TuplePrefix↔TuplePrefix cannot rebind — write through the
        // root var when we still have it.
        match recv_ty {
            Type::Var(v) => {
                if occurs(v, &extended) {
                    return Err(at(
                        span,
                        format!(
                            "recursive type: a type variable occurs inside {}",
                            crate::display::display_type(&extended, &[])
                        ),
                    ));
                }
                self.uni.subst.insert(v, extended);
            }
            other => self.unify_at(span, other, extended)?,
        }
        Ok(field_ty)
    }

    fn constrain_open_product_receiver(
        &mut self,
        span: lumia_syntax::Span,
        recv_ty: Type,
        expect_adt: Option<&str>,
        idx: usize,
    ) -> Result<Type, TypeError> {
        // Constrain open receivers when the expected product/ctor is known
        // (named `.field` / Some/Ok/Err patterns). Leaving them open let
        // `{ p -> p.x }(1)` typecheck and crash at runtime.
        if let Some(want) = expect_adt {
            if want == "Ok" || want == "Err" {
                let t = self.fresh();
                let e = self.fresh();
                self.unify_at(
                    span,
                    recv_ty,
                    Type::Adt {
                        name: lumia_hir::RESULT.name.into(),
                        params: vec![t.clone(), e.clone()],
                    },
                )?;
                return Ok(if want == "Ok" { t } else { e });
            }
            if want == "Some" {
                let t = self.fresh();
                self.unify_at(
                    span,
                    recv_ty,
                    Type::Adt {
                        name: lumia_hir::OPTION.name.into(),
                        params: vec![t.clone()],
                    },
                )?;
                return Ok(t);
            }
            if let Some((adt, var_arity, offset)) = self.products.sum_ctors.get(want).cloned() {
                if idx >= var_arity {
                    return Err(at(
                        span,
                        format!(
                            "variant `{want}` has {var_arity} field(s); index {idx} out of range"
                        ),
                    ));
                }
                let rec = self
                    .products
                    .sum_field_recursive
                    .get(want)
                    .and_then(|v| v.get(idx).copied())
                    .unwrap_or(false);
                let total = self
                    .products
                    .sum_max_arity
                    .get(&adt)
                    .copied()
                    .unwrap_or(0);
                let params: Vec<Type> = (0..total).map(|_| self.fresh()).collect();
                let adt_ty = Type::Adt {
                    name: adt.clone(),
                    params: params.clone(),
                };
                if rec {
                    self.unify_at(span, recv_ty, adt_ty.clone())?;
                    return Ok(adt_ty);
                }
                let local = (0..idx)
                    .filter(|&i| {
                        !self
                            .products
                            .sum_field_recursive
                            .get(want)
                            .and_then(|v| v.get(i).copied())
                            .unwrap_or(false)
                    })
                    .count();
                let slot = offset + local;
                let field_ty = params.get(slot).cloned().ok_or_else(|| {
                    at(
                        span,
                        format!(
                            "variant `{want}` field #{idx} has no type parameter slot on `{adt}`"
                        ),
                    )
                })?;
                self.unify_at(span, recv_ty, adt_ty)?;
                return Ok(field_ty);
            }
            let arity = self
                .products
                .products
                .get(want)
                .map(|fs| fs.len())
                .unwrap_or(idx + 1)
                .max(idx + 1);
            let params: Vec<Type> = (0..arity).map(|_| self.fresh()).collect();
            let field_ty = params[idx].clone();
            self.unify_at(
                span,
                recv_ty,
                Type::Adt {
                    name: want.into(),
                    params,
                },
            )?;
            return Ok(field_ty);
        }
        // Positional `.0`/`.n`: constrain to an open tuple prefix of
        // length `idx+1` (at-least-N). Unifies with longer tuples
        // (pairs for `{ t -> t.0 }` / sortBy) but rejects `Int` etc.
        let prefix: Vec<Type> = (0..=idx).map(|_| self.fresh()).collect();
        let field_ty = prefix[idx].clone();
        self.unify_at(span, recv_ty, Type::TuplePrefix(prefix))?;
        Ok(field_ty)
    }
}

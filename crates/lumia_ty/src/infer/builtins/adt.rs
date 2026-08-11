//! BuiltinCall typing — adt family.

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
        // 2 args: tuple/positional `.0`; 3 args: product field with expected ADT name.
        if args.len() != 2 && args.len() != 3 {
            return Err(at(span, "adt_field takes 2 or 3 arguments"));
        }
        let (recv_ty, ae) = self.infer_expr(&args[0])?;
        let (it, ie) = self.infer_expr(&args[1])?;
        self.unify_at(span, it, Type::Int)?;
        let mut eff = self.union_eff(ae, ie);
        let expect_adt = if args.len() == 3 {
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
            if name == "Result" && (want == "Ok" || want == "Err") {
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
            if name == "Option" && want == "Some" {
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
                return Err(at(
                    span,
                    format!("field projection expects type `{want}`, got `{name}`"),
                ));
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
        self.unify_at(span, recv_ty, Type::TuplePrefix(prefix))?;
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
                        name: "Result".into(),
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
                        name: "Option".into(),
                        params: vec![t.clone()],
                    },
                )?;
                return Ok(t);
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

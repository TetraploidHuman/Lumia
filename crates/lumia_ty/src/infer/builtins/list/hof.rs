//! List HOF / parallel: sortByKeys, parMap, parFold.

use super::super::super::Infer;
use crate::types::{at, expr_span, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(super) fn infer_list_hof(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::ListSortByKeys => {
                let (vt, ve) = self.infer_expr(&args[0])?;
                let (kt, ke) = self.infer_expr(&args[1])?;
                let elem = self.expect_list_elem(vt, span, "sortBy")?;
                match self.prune(kt) {
                    Type::List(t) => {
                        let t = self.prune(*t);
                        match t {
                            Type::Int | Type::String | Type::Char => {}
                            Type::Var(_) => {}
                            other => {
                                return Err(at(span, format!(
                                        "sortBy keys: expected List[Int|String|Char], got List[{other:?}]"
                                    )));
                            }
                        }
                    }
                    Type::Var(_) => {}
                    other => {
                        return Err(at(
                            span,
                            format!("sortBy keys: expected List, got {other:?}"),
                        ));
                    }
                }
                Ok((Type::List(Box::new(elem)), self.union_eff(ve, ke)))
            }
            Builtin::ListParMap => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (ft, fe) = self.infer_expr(&args[1])?;
                let elem = self.expect_list_elem(lt, span, "map")?;
                let out = self.fresh();
                let cb_eff = self.callback_effect(ft.clone());
                self.unify_at(
                    span,
                    ft,
                    Type::Fun(vec![elem], Box::new(out.clone()), cb_eff),
                )?;
                let out = self.prune(out);
                let eff = self.union_eff(fe, cb_eff);
                let eff = self.union_eff(le, eff);
                Ok((Type::List(Box::new(out)), eff))
            }
            Builtin::ListParFold => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                let elem = self.expect_list_elem(lt, span, "fold")?;
                let acc = self.prune(it);
                let (ft, fe) = match &args[2] {
                    Expr::Lambda {
                        params,
                        body,
                        span: lsp,
                        ..
                    } if params.len() == 2 => {
                        self.push();
                        self.bind(params[0].clone(), acc.clone());
                        self.bind(params[1].clone(), elem.clone());
                        let (rt, re) = self.infer_expr(body)?;
                        self.pop();
                        self.unify_at(*lsp, rt, acc.clone())?;
                        let ft =
                            Type::Fun(vec![acc.clone(), elem.clone()], Box::new(acc.clone()), re);
                        self.type_at.push((expr_span(&args[2]), ft.clone()));
                        (ft, Effect::pure())
                    }
                    _ => self.infer_expr(&args[2])?,
                };
                let cb_eff = self.callback_effect(ft.clone());
                self.unify_at(
                    span,
                    ft,
                    Type::Fun(vec![acc.clone(), elem], Box::new(acc.clone()), cb_eff),
                )?;
                let acc = self.prune(acc);
                let eff = self.union_eff(fe, cb_eff);
                let eff = self.union_eff(le, eff);
                let eff = self.union_eff(ie, eff);
                Ok((acc, eff))
            }
            _ => unreachable!("infer_list_hof"),
        }
    }
}

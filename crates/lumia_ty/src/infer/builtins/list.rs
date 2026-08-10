//! BuiltinCall typing — list family.

use super::super::Infer;
use crate::types::{at, expr_span, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_list_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::ListLen => {
                let (t, e) = self.infer_expr(&args[0])?;
                let t = self.prune(t);
                match t {
                    Type::List(_) | Type::Set(_) | Type::Map(_, _) | Type::String => {}
                    Type::Var(_) => {
                        // Unconstrained: treat as List (match desugar / polymorphic use).
                        let elem = self.fresh();
                        self.unify_at(span, t, Type::List(Box::new(elem)))?;
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("len: expected List/Set/Map/String, got {other:?}"),
                        ));
                    }
                }
                Ok((Type::Int, e))
            }
            Builtin::ListGet => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                let lt_p = self.prune(lt);
                let elem = match lt_p {
                    Type::List(t) => {
                        self.unify_at(span, it, Type::Int)?;
                        *t
                    }
                    Type::Set(t) => {
                        self.unify_at(span, it, Type::Int)?;
                        *t
                    }
                    Type::Map(k, v) => {
                        self.unify_at(span, it, *k)?;
                        Type::Adt {
                            name: "Option".into(),
                            params: vec![*v],
                        }
                    }
                    Type::Var(v) => {
                        // Default to List (match desugar); Map is typed from mapOf.
                        self.unify_at(span, it, Type::Int)?;
                        let elem = self.fresh();
                        self.unify_at(span, Type::Var(v), Type::List(Box::new(elem.clone())))?;
                        elem
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("get: expected List, Set, or Map, got {other:?}"),
                        ));
                    }
                };
                Ok((elem, self.union_eff(le, ie)))
            }
            Builtin::Elems => {
                let (ct, ce) = self.infer_expr(&args[0])?;
                let list_ty = match self.prune(ct.clone()) {
                    Type::List(e) => Type::List(e),
                    Type::Set(e) => Type::List(e),
                    Type::Map(k, _) => Type::List(k),
                    Type::Var(_) => {
                        let e = self.fresh();
                        self.unify_at(span, ct, Type::List(Box::new(e.clone())))?;
                        Type::List(Box::new(e))
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("elems: expected List, Set, or Map, got {other:?}"),
                        ));
                    }
                };
                Ok((list_ty, ce))
            }
            Builtin::ListSlice => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                self.unify_at(span, it, Type::Int)?;
                let elem = match self.prune(lt.clone()) {
                    Type::List(t) => t,
                    Type::Var(_) => {
                        let elem = self.fresh();
                        self.unify_at(span, lt, Type::List(Box::new(elem.clone())))?;
                        Box::new(elem)
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("slice/drop: expected List, got {other:?}"),
                        ));
                    }
                };
                Ok((Type::List(elem), self.union_eff(le, ie)))
            }
            Builtin::ListTake => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                self.unify_at(span, it, Type::Int)?;
                let elem = match self.prune(lt.clone()) {
                    Type::List(t) => t,
                    Type::Var(_) => {
                        let elem = self.fresh();
                        self.unify_at(span, lt, Type::List(Box::new(elem.clone())))?;
                        Box::new(elem)
                    }
                    other => {
                        return Err(at(span, format!("take: expected List, got {other:?}")));
                    }
                };
                Ok((Type::List(elem), self.union_eff(le, ie)))
            }
            Builtin::ListReverse => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let elem = match self.prune(lt.clone()) {
                    Type::List(t) => t,
                    Type::Var(_) => {
                        let elem = self.fresh();
                        self.unify_at(span, lt, Type::List(Box::new(elem.clone())))?;
                        Box::new(elem)
                    }
                    other => {
                        return Err(at(span, format!("reverse: expected List, got {other:?}")));
                    }
                };
                Ok((Type::List(elem), le))
            }
            Builtin::ListSort => {
                let (lt, le) = self.infer_expr(&args[0])?;
                match self.prune(lt.clone()) {
                    Type::List(t) => {
                        self.unify_at(span, *t, Type::Int)?;
                    }
                    Type::Var(_) => {
                        self.unify_at(span, lt, Type::List(Box::new(Type::Int)))?;
                    }
                    other => {
                        return Err(at(span, format!("sort: expected List[Int], got {other:?}")));
                    }
                }
                Ok((Type::List(Box::new(Type::Int)), le))
            }
            Builtin::ListSortByKeys => {
                let (vt, ve) = self.infer_expr(&args[0])?;
                let (kt, ke) = self.infer_expr(&args[1])?;
                let elem = match self.prune(vt.clone()) {
                    Type::List(t) => *t,
                    Type::Var(_) => {
                        let e = self.fresh();
                        self.unify_at(span, vt, Type::List(Box::new(e.clone())))?;
                        e
                    }
                    other => {
                        return Err(at(span, format!("sortBy: expected List, got {other:?}")));
                    }
                };
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
                    Type::Var(_) => {
                        // Key type filled by the key function; leave open.
                    }
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
                // FunRef-safe shape from lower; may be demoted after infer if
                // impure or non-scalar (see `finalize_auto_parallel`).
                let (lt, le) = self.infer_expr(&args[0])?;
                let (ft, fe) = self.infer_expr(&args[1])?;
                let elem = match self.prune(lt.clone()) {
                    Type::List(t) => *t,
                    Type::Var(_) => {
                        let e = self.fresh();
                        self.unify_at(span, lt, Type::List(Box::new(e.clone())))?;
                        e
                    }
                    other => {
                        return Err(at(span, format!("map: expected List, got {other:?}")));
                    }
                };
                let out = self.fresh();
                let cb_eff = match self.prune(ft.clone()) {
                    Type::Fun(_, _, e) => self.prune_eff(e),
                    _ => Effect::pure(),
                };
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
                // FunRef-safe shape from lower; may be demoted after infer if
                // impure or non-scalar (see `finalize_auto_parallel`).
                // Infer init/list first so lambda params are not free Vars
                // (otherwise `acc.get` defaults to List and breaks Map folds).
                let (lt, le) = self.infer_expr(&args[0])?;
                let (it, ie) = self.infer_expr(&args[1])?;
                let elem = match self.prune(lt.clone()) {
                    Type::List(t) => *t,
                    Type::Var(_) => {
                        let e = self.fresh();
                        self.unify_at(span, lt, Type::List(Box::new(e.clone())))?;
                        e
                    }
                    other => {
                        return Err(at(span, format!("fold: expected List, got {other:?}")));
                    }
                };
                let acc = self.prune(it);
                let (ft, fe) = match &args[2] {
                    Expr::Lambda {
                        params,
                        body,
                        span: lsp,
                    } if params.len() == 2 => {
                        self.push();
                        self.bind(params[0].clone(), acc.clone());
                        self.bind(params[1].clone(), elem.clone());
                        let (rt, re) = self.infer_expr(body)?;
                        self.pop();
                        self.unify_at(*lsp, rt, acc.clone())?;
                        let ft =
                            Type::Fun(vec![acc.clone(), elem.clone()], Box::new(acc.clone()), re);
                        // Mirror `infer_expr` type_at for finalize_auto_parallel.
                        self.type_at.push((expr_span(&args[2]), ft.clone()));
                        (ft, Effect::pure())
                    }
                    _ => self.infer_expr(&args[2])?,
                };
                let cb_eff = match self.prune(ft.clone()) {
                    Type::Fun(_, _, e) => self.prune_eff(e),
                    _ => Effect::pure(),
                };
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
            Builtin::ListJoin => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (st, se) = self.infer_expr(&args[1])?;
                self.unify_at(span, st, Type::String)?;
                match self.prune(lt.clone()) {
                    Type::List(t) => self.unify_at(span, *t, Type::String)?,
                    Type::Var(_) => {
                        self.unify_at(span, lt, Type::List(Box::new(Type::String)))?;
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("join: expected List[String], got {other:?}"),
                        ));
                    }
                }
                Ok((Type::String, self.union_eff(le, se)))
            }
            Builtin::ListAppend => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (et, ee) = self.infer_expr(&args[1])?;
                let list_ty = match self.prune(lt.clone()) {
                    Type::List(t) => {
                        self.unify_at(span, et, *t.clone())?;
                        Type::List(t)
                    }
                    Type::Var(_) => {
                        self.unify_at(span, lt, Type::List(Box::new(et.clone())))?;
                        Type::List(Box::new(et))
                    }
                    other => {
                        return Err(at(span, format!("append: expected List, got {other:?}")));
                    }
                };
                Ok((list_ty, self.union_eff(le, ee)))
            }
            Builtin::ListConcat => {
                let (lt, le) = self.infer_expr(&args[0])?;
                let (rt, re) = self.infer_expr(&args[1])?;
                let lt = self.prune(lt);
                let rt = self.prune(rt);
                match (&lt, &rt) {
                    (Type::String, Type::String)
                    | (Type::String, Type::Var(_))
                    | (Type::Var(_), Type::String) => {
                        self.unify_at(span, lt, Type::String)?;
                        self.unify_at(span, rt, Type::String)?;
                        Ok((Type::String, self.union_eff(le, re)))
                    }
                    _ => {
                        let list_ty = match (lt.clone(), rt.clone()) {
                            (Type::List(a), Type::List(b)) => {
                                self.unify_at(span, *a.clone(), *b)?;
                                Type::List(a)
                            }
                            (Type::List(a), Type::Var(_)) => {
                                self.unify_at(span, rt, Type::List(a.clone()))?;
                                Type::List(a)
                            }
                            (Type::Var(_), Type::List(b)) => {
                                self.unify_at(span, lt, Type::List(b.clone()))?;
                                Type::List(b)
                            }
                            (Type::Var(_), Type::Var(_)) => {
                                let elem = self.fresh();
                                let list = Type::List(Box::new(elem));
                                self.unify_at(span, lt, list.clone())?;
                                self.unify_at(span, rt, list.clone())?;
                                list
                            }
                            (other, _) => {
                                return Err(at(
                                    span,
                                    format!("concat: expected List or String, got {other:?}"),
                                ));
                            }
                        };
                        Ok((list_ty, self.union_eff(le, re)))
                    }
                }
            }
            Builtin::Range | Builtin::RangeInclusive => {
                let mut eff = Effect::pure();
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    self.unify_at(span, t, Type::Int)?;
                    eff = self.union_eff(eff, e);
                }
                Ok((Type::List(Box::new(Type::Int)), eff))
            }
            _ => unreachable!("non-list builtin dispatched to infer_list_builtin"),
        }
    }
}

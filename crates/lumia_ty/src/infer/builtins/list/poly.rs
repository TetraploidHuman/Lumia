//! ListLen / ListGet / Elems (polymorphic over List/Set/Map/String).

use super::super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(super) fn infer_list_poly(
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
                    Type::Var(v) => {
                        // Do not default open receivers to List — String/Set/Map
                        // also support `.len()` (see Scheme::len_vars).
                        self.uni.len_vars.insert(v);
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
                let lt_p = self.prune(lt.clone());
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
                    Type::Var(_) => {
                        // Under `alt` → Map/`Option` so `getOr = { m,k,d -> m.get(k) alt d }`
                        // works. Otherwise List element (Int index) so poly
                        // `pts.get(i) + …` keeps working without `concat(listOf())`.
                        if self.ctrl.alt_scrutinee_depth > 0 {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify_at(
                                span,
                                lt,
                                Type::Map(Box::new(k.clone()), Box::new(v.clone())),
                            )?;
                            self.unify_at(span, it, k)?;
                            Type::Adt {
                                name: "Option".into(),
                                params: vec![v],
                            }
                        } else {
                            self.unify_at(span, it, Type::Int)?;
                            let elem = self.fresh();
                            self.unify_at(span, lt, Type::List(Box::new(elem.clone())))?;
                            elem
                        }
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
                    Type::Var(v) => {
                        // Do not default open receivers to List — Set/Map
                        // `.toList()` / for-in use Elems too.
                        self.uni.elems_vars.insert(v);
                        let e = self.fresh();
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
            _ => unreachable!("infer_list_poly"),
        }
    }
}

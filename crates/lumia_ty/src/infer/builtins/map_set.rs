//! BuiltinCall typing — map_set family.

use super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    /// Expect `ty` to be `Map[k,v]`, or constrain a Var to a fresh Map. Returns `(k, v)`.
    fn map_kv_from_receiver(
        &mut self,
        mt: Type,
        span: lumia_syntax::Span,
        op: &str,
    ) -> Result<(Type, Type), TypeError> {
        match self.prune(mt.clone()) {
            Type::Map(k, v) => Ok((*k, *v)),
            Type::Var(_) => {
                let k = self.fresh();
                let v = self.fresh();
                self.unify_at(
                    span,
                    mt,
                    Type::Map(Box::new(k.clone()), Box::new(v.clone())),
                )?;
                Ok((k, v))
            }
            other => Err(at(span, format!("{op}: expected Map, got {other:?}"))),
        }
    }

    pub(crate) fn infer_map_set_builtin(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::Contains => {
                let (ct, ce) = self.infer_expr(&args[0])?;
                let (kt, ke) = self.infer_expr(&args[1])?;
                match self.prune(ct) {
                    Type::Map(k, _) => self.unify_at(span, kt, *k)?,
                    Type::Set(e) => self.unify_at(span, kt, *e)?,
                    Type::String => self.unify_at(span, kt, Type::String)?,
                    Type::Var(_) => {
                        // Leave open so later use can unify with Set/Map/String.
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("contains: expected Map, Set, or String, got {other:?}"),
                        ));
                    }
                }
                Ok((Type::Bool, self.union_eff(ce, ke)))
            }
            Builtin::MapSet => {
                let (mt, me) = self.infer_expr(&args[0])?;
                let (kt, ke) = self.infer_expr(&args[1])?;
                let (vt, ve) = self.infer_expr(&args[2])?;
                match self.prune(mt.clone()) {
                    Type::Map(k, v) => {
                        self.unify_at(span, kt, *k.clone())?;
                        self.unify_at(span, vt, *v.clone())?;
                        Ok((Type::Map(k, v), self.union3_eff(me, ke, ve)))
                    }
                    Type::List(elem) => {
                        self.unify_at(span, kt, Type::Int)?;
                        self.unify_at(span, vt, *elem.clone())?;
                        Ok((Type::List(elem), self.union3_eff(me, ke, ve)))
                    }
                    Type::Var(_) => {
                        // Prefer Map when unconstrained (UFCS `.set` on maps).
                        self.unify_at(
                            span,
                            mt,
                            Type::Map(Box::new(kt.clone()), Box::new(vt.clone())),
                        )?;
                        Ok((
                            Type::Map(Box::new(kt), Box::new(vt)),
                            self.union3_eff(me, ke, ve),
                        ))
                    }
                    other => Err(at(
                        span,
                        format!("set: expected Map or List, got {other:?}"),
                    )),
                }
            }
            Builtin::MapRemove => {
                let (mt, me) = self.infer_expr(&args[0])?;
                let (kt, ke) = self.infer_expr(&args[1])?;
                match self.prune(mt.clone()) {
                    Type::Map(k, v) => {
                        self.unify_at(span, kt, *k.clone())?;
                        Ok((Type::Map(k, v), self.union_eff(me, ke)))
                    }
                    Type::Set(e) => {
                        self.unify_at(span, kt, *e.clone())?;
                        Ok((Type::Set(e), self.union_eff(me, ke)))
                    }
                    Type::Var(_) => {
                        // Keep open; call site / later use constrains Map vs Set.
                        Ok((mt, self.union_eff(me, ke)))
                    }
                    other => Err(at(
                        span,
                        format!("remove: expected Map or Set, got {other:?}"),
                    )),
                }
            }
            Builtin::SetInsert => {
                let (st, se) = self.infer_expr(&args[0])?;
                let (et, ee) = self.infer_expr(&args[1])?;
                match self.prune(st.clone()) {
                    Type::Set(e) => {
                        self.unify_at(span, et, *e.clone())?;
                        Ok((Type::Set(e), self.union_eff(se, ee)))
                    }
                    Type::Var(_) => {
                        self.unify_at(span, st, Type::Set(Box::new(et.clone())))?;
                        Ok((Type::Set(Box::new(et)), self.union_eff(se, ee)))
                    }
                    other => Err(at(span, format!("insert: expected Set, got {other:?}"))),
                }
            }
            Builtin::MapKeys => {
                let (mt, me) = self.infer_expr(&args[0])?;
                let (k, _) = self.map_kv_from_receiver(mt, span, "keys")?;
                Ok((Type::List(Box::new(k)), me))
            }
            Builtin::MapValues => {
                let (mt, me) = self.infer_expr(&args[0])?;
                let (_, v) = self.map_kv_from_receiver(mt, span, "values")?;
                Ok((Type::List(Box::new(v)), me))
            }
            Builtin::MapItems => {
                let (mt, me) = self.infer_expr(&args[0])?;
                // Map → List[(K,V)]; already a List of pairs → identity (for-in sugar).
                let pair_list = match self.prune(mt.clone()) {
                    Type::Map(k, v) => Type::List(Box::new(Type::Tuple(vec![*k, *v]))),
                    Type::List(elem) => {
                        let elem = self.prune(*elem);
                        match elem {
                            Type::Tuple(ts) if ts.len() == 2 => {
                                Type::List(Box::new(Type::Tuple(ts)))
                            }
                            Type::Adt { name, params }
                                if (name == "__Tuple" || name.is_empty()) && params.len() == 2 =>
                            {
                                Type::List(Box::new(Type::Tuple(params)))
                            }
                            Type::Var(_) => {
                                let k = self.fresh();
                                let v = self.fresh();
                                let pair = Type::Tuple(vec![k, v]);
                                self.unify_at(
                                    span,
                                    Type::List(Box::new(elem)),
                                    Type::List(Box::new(pair.clone())),
                                )?;
                                Type::List(Box::new(pair))
                            }
                            other => {
                                return Err(at(
                                    span,
                                    format!(
                                        "items: expected Map or List of pairs, got List({other:?})"
                                    ),
                                ));
                            }
                        }
                    }
                    Type::Var(_) => {
                        let k = self.fresh();
                        let v = self.fresh();
                        self.unify_at(
                            span,
                            mt,
                            Type::Map(Box::new(k.clone()), Box::new(v.clone())),
                        )?;
                        Type::List(Box::new(Type::Tuple(vec![k, v])))
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("items: expected Map or List of pairs, got {other:?}"),
                        ));
                    }
                };
                Ok((pair_list, me))
            }
            _ => unreachable!("non-map_set builtin dispatched to infer_map_set_builtin"),
        }
    }
}

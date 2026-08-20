//! BuiltinCall typing — map_set family.

use super::super::Infer;
use crate::types::{at, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};
use std::sync::Arc;

impl Infer {
    /// Expect `ty` to be `Map[k,v]`, or constrain a Var to a fresh Map. Returns `(k, v)`.
    fn map_kv_from_receiver(
        &mut self,
        mt: Type,
        span: lumia_syntax::Span,
        op: &str,
    ) -> Result<(Type, Type), TypeError> {
        match self.prune(mt.clone()) {
            Type::Map(k, v) => Ok((Type::unbox(k), Type::unbox(v))),
            Type::Var(_) => {
                let k = self.fresh();
                let v = self.fresh();
                self.unify_at(
                    span,
                    mt,
                    Type::Map(Arc::new(k.clone()), Arc::new(v.clone())),
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
                    Type::Map(k, _) => self.unify_at(span, kt, Type::unbox(k))?,
                    Type::Set(e) => self.unify_at(span, kt, Type::unbox(e))?,
                    Type::String => self.unify_at(span, kt, Type::String)?,
                    Type::Var(v) => {
                        // Leave open so later use can unify with Set/Map/String
                        // (not List — RT has no list contains).
                        self.uni.contains_vars.insert(v);
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
                        self.unify_at(span, kt, Type::unbox(k.clone()))?;
                        self.unify_at(span, vt, Type::unbox(v.clone()))?;
                        Ok((Type::Map(k, v), self.union3_eff(me, ke, ve)))
                    }
                    Type::List(elem) => {
                        self.unify_at(span, kt, Type::Int)?;
                        self.unify_at(span, vt, Type::unbox(elem.clone()))?;
                        Ok((Type::List(elem), self.union3_eff(me, ke, ve)))
                    }
                    Type::Var(v) => {
                        // Keep open — Int-key List update vs Map upsert is decided
                        // when the receiver is bound (see set_vars).
                        self.uni.set_vars.insert(v);
                        Ok((mt, self.union3_eff(me, ke, ve)))
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
                        self.unify_at(span, kt, Type::unbox(k.clone()))?;
                        Ok((Type::Map(k, v), self.union_eff(me, ke)))
                    }
                    Type::Set(e) => {
                        self.unify_at(span, kt, Type::unbox(e.clone()))?;
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
                        self.unify_at(span, et, Type::unbox(e.clone()))?;
                        Ok((Type::Set(e), self.union_eff(se, ee)))
                    }
                    Type::Var(_) => {
                        self.unify_at(span, st, Type::Set(Arc::new(et.clone())))?;
                        Ok((Type::Set(Arc::new(et)), self.union_eff(se, ee)))
                    }
                    other => Err(at(span, format!("insert: expected Set, got {other:?}"))),
                }
            }
            Builtin::SetUnion | Builtin::SetIntersect | Builtin::SetDiff => {
                let (at0, ae) = self.infer_expr(&args[0])?;
                let (bt, be) = self.infer_expr(&args[1])?;
                let op = match name {
                    Builtin::SetUnion => "union",
                    Builtin::SetIntersect => "intersect",
                    _ => "diff",
                };
                let elem = match self.prune(at0.clone()) {
                    Type::Set(e) => Type::unbox(e),
                    Type::Var(_) => {
                        let e = self.fresh();
                        self.unify_at(span, at0, Type::Set(Arc::new(e.clone())))?;
                        e
                    }
                    other => {
                        return Err(at(span, format!("{op}: expected Set, got {other:?}")));
                    }
                };
                self.unify_at(span, bt, Type::Set(Arc::new(elem.clone())))?;
                Ok((Type::Set(Arc::new(elem)), self.union_eff(ae, be)))
            }
            Builtin::MapKeys => {
                let (mt, me) = self.infer_expr(&args[0])?;
                let (k, _) = self.map_kv_from_receiver(mt, span, "keys")?;
                Ok((Type::List(Arc::new(k)), me))
            }
            Builtin::MapValues => {
                let (mt, me) = self.infer_expr(&args[0])?;
                let (_, v) = self.map_kv_from_receiver(mt, span, "values")?;
                Ok((Type::List(Arc::new(v)), me))
            }
            Builtin::MapItems => {
                let (mt, me) = self.infer_expr(&args[0])?;
                // Map → List[(K,V)]; already a List of pairs → identity (for-in sugar).
                let pair_list = match self.prune(mt.clone()) {
                    Type::Map(k, v) => Type::List(Arc::new(Type::Tuple(vec![Type::unbox(k), Type::unbox(v)]))),
                    Type::List(elem) => {
                        let elem = self.prune(Type::unbox(elem));
                        match elem {
                            Type::Tuple(ts) if ts.len() == 2 => {
                                Type::List(Arc::new(Type::Tuple(ts)))
                            }
                            Type::Adt { name, params }
                                if (name == "__Tuple" || name.is_empty()) && params.len() == 2 =>
                            {
                                Type::List(Arc::new(Type::Tuple(params)))
                            }
                            Type::Var(_) => {
                                let k = self.fresh();
                                let v = self.fresh();
                                let pair = Type::Tuple(vec![k, v]);
                                self.unify_at(
                                    span,
                                    Type::List(Arc::new(elem)),
                                    Type::List(Arc::new(pair.clone())),
                                )?;
                                Type::List(Arc::new(pair))
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
                            Type::Map(Arc::new(k.clone()), Arc::new(v.clone())),
                        )?;
                        Type::List(Arc::new(Type::Tuple(vec![k, v])))
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

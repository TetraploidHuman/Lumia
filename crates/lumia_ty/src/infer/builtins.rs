//! BuiltinCall typing.

use super::Infer;
use crate::types::{at, expr_span, Effect, Type, TypeError};
use lumia_hir::{Builtin, Expr};

impl Infer {
    pub(crate) fn infer_builtin_call(
        &mut self,
        name: &Builtin,
        args: &[Expr],
        span: lumia_syntax::Span,
    ) -> Result<(Type, Effect), TypeError> {
        match name {
            Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                if args.len() != 1 {
                    return Err(at(span, "println takes 1 argument"));
                }
                let (t, e) = self.infer_expr(&args[0])?;
                let t = self.prune(t);
                match t {
                    Type::Int
                    | Type::String
                    | Type::Bool
                    | Type::Float
                    | Type::Char
                    | Type::Adt { .. }
                    | Type::List(_)
                    | Type::Map(_, _)
                    | Type::Set(_)
                    | Type::Tuple(_) => {}
                    Type::Var(_) => {
                        // Leave open: freezing to Int rejected `f(1.5)` for
                        // `{ x -> println(x); x }` and poisoned later Float uses.
                        // Unresolved vars still print via `println_auto` (Int default).
                    }
                    other => {
                        return Err(at(span, format!("println: unsupported type {other:?}")));
                    }
                }
                Ok((Type::Unit, self.union_eff(Effect::io(), e)))
            }
            Builtin::ListLen => {
                if args.len() != 1 {
                    return Err(at(span, "len takes 1 argument"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "get takes 2 arguments"));
                }
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
            Builtin::Contains => {
                if args.len() != 2 {
                    return Err(at(span, "contains takes 2 arguments"));
                }
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
                if args.len() != 3 {
                    return Err(at(
                        span,
                        "set takes 3 arguments (map/list, key/index, value)",
                    ));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "remove takes 2 arguments"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "insert takes 2 arguments"));
                }
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
            Builtin::Elems => {
                if args.len() != 1 {
                    return Err(at(span, "elems takes 1 argument"));
                }
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
            Builtin::MapKeys => {
                if args.len() != 1 {
                    return Err(at(span, "keys takes 1 argument"));
                }
                let (mt, me) = self.infer_expr(&args[0])?;
                let k = match self.prune(mt.clone()) {
                    Type::Map(k, _) => *k,
                    Type::Var(_) => {
                        let k = self.fresh();
                        let v = self.fresh();
                        self.unify_at(span, mt, Type::Map(Box::new(k.clone()), Box::new(v)))?;
                        k
                    }
                    other => {
                        return Err(at(span, format!("keys: expected Map, got {other:?}")));
                    }
                };
                Ok((Type::List(Box::new(k)), me))
            }
            Builtin::MapValues => {
                if args.len() != 1 {
                    return Err(at(span, "values takes 1 argument"));
                }
                let (mt, me) = self.infer_expr(&args[0])?;
                let v = match self.prune(mt.clone()) {
                    Type::Map(_, v) => *v,
                    Type::Var(_) => {
                        let k = self.fresh();
                        let v = self.fresh();
                        self.unify_at(span, mt, Type::Map(Box::new(k), Box::new(v.clone())))?;
                        v
                    }
                    other => {
                        return Err(at(span, format!("values: expected Map, got {other:?}")));
                    }
                };
                Ok((Type::List(Box::new(v)), me))
            }
            Builtin::MapItems => {
                if args.len() != 1 {
                    return Err(at(span, "items takes 1 argument"));
                }
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
            Builtin::AdtTag => {
                if args.len() != 1 {
                    return Err(at(span, "adt_tag takes 1 argument"));
                }
                let (_, e) = self.infer_expr(&args[0])?;
                Ok((Type::Int, e))
            }
            Builtin::AdtField => {
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
                        if let Some(want) = expect_adt {
                            // Variant patterns pass ctor name (`Ok`/`Err`/`Some`);
                            // product patterns / field proj pass the ADT name.
                            if name == "Result" && (want == "Ok" || want == "Err") {
                                if idx != 0 {
                                    return Err(at(
                                            span,
                                            format!(
                                                "Result::{want} has a single payload (index 0), got {idx}"
                                            ),
                                        ));
                                }
                                let pi = if want == "Ok" { 0 } else { 1 };
                                return Ok((
                                    params.get(pi).cloned().ok_or_else(|| {
                                        at(
                                            span,
                                            format!(
                                                "Result::{want} payload missing (params {})",
                                                params.len()
                                            ),
                                        )
                                    })?,
                                    eff,
                                ));
                            }
                            if name == "Option" && want == "Some" {
                                if idx != 0 {
                                    return Err(at(
                                            span,
                                            format!(
                                                "Option::Some has a single payload (index 0), got {idx}"
                                            ),
                                        ));
                                }
                                return Ok((
                                    params
                                        .first()
                                        .cloned()
                                        .ok_or_else(|| at(span, "Option::Some payload missing"))?,
                                    eff,
                                ));
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
                        })?
                    }
                    Type::Tuple(ts) => {
                        if expect_adt.is_some() {
                            return Err(at(span, "named product field applied to a tuple"));
                        }
                        ts.get(idx).cloned().ok_or_else(|| {
                            at(
                                span,
                                format!("tuple index {idx} out of range (arity {})", ts.len()),
                            )
                        })?
                    }
                    Type::TuplePrefix(ts) => {
                        if expect_adt.is_some() {
                            return Err(at(span, "named product field applied to a tuple"));
                        }
                        if idx < ts.len() {
                            ts[idx].clone()
                        } else {
                            // Extend prefix to cover this index.
                            let mut prefix = ts;
                            while prefix.len() <= idx {
                                prefix.push(self.fresh());
                            }
                            let field_ty = prefix[idx].clone();
                            self.unify_at(span, recv_ty, Type::TuplePrefix(prefix))?;
                            field_ty
                        }
                    }
                    Type::Var(_) => {
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
                                if want == "Ok" {
                                    t
                                } else {
                                    e
                                }
                            } else if want == "Some" {
                                let t = self.fresh();
                                self.unify_at(
                                    span,
                                    recv_ty,
                                    Type::Adt {
                                        name: "Option".into(),
                                        params: vec![t.clone()],
                                    },
                                )?;
                                t
                            } else {
                                let arity = self
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
                                field_ty
                            }
                        } else {
                            // Positional `.0`/`.n`: constrain to an open tuple prefix of
                            // length `idx+1` (at-least-N). Unifies with longer tuples
                            // (pairs for `{ t -> t.0 }` / sortBy) but rejects `Int` etc.
                            let prefix: Vec<Type> = (0..=idx).map(|_| self.fresh()).collect();
                            let field_ty = prefix[idx].clone();
                            self.unify_at(span, recv_ty, Type::TuplePrefix(prefix))?;
                            field_ty
                        }
                    }
                    other => {
                        return Err(at(
                            span,
                            format!("field projection: expected product/tuple, got {other:?}"),
                        ));
                    }
                };
                Ok((elem, eff))
            }
            Builtin::ListSlice => {
                if args.len() != 2 {
                    return Err(at(span, "slice/drop takes 2 arguments"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "take takes 2 arguments"));
                }
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
                if args.len() != 1 {
                    return Err(at(span, "reverse takes 1 argument"));
                }
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
                if args.len() != 1 {
                    return Err(at(span, "sort takes 1 argument"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "sortByKeys takes 2 arguments (values, keys)"));
                }
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
                match self.prune(kt.clone()) {
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
                if args.len() != 2 {
                    return Err(at(span, "map takes 2 arguments"));
                }
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
                if args.len() != 3 {
                    return Err(at(span, "fold takes 3 arguments"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "join takes 2 arguments (list, separator)"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "append takes 2 arguments"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "concat takes 2 arguments"));
                }
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
                if args.len() != 2 {
                    return Err(at(span, "range takes 2 arguments"));
                }
                let mut eff = Effect::pure();
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    self.unify_at(span, t, Type::Int)?;
                    eff = self.union_eff(eff, e);
                }
                Ok((Type::List(Box::new(Type::Int)), eff))
            }
            Builtin::Show => {
                if args.len() != 1 {
                    return Err(at(span, "show takes 1 argument"));
                }
                let (_, e) = self.infer_expr(&args[0])?;
                Ok((Type::String, e))
            }
            Builtin::StrTrim | Builtin::StrToLower | Builtin::StrToUpper => {
                if args.len() != 1 {
                    return Err(at(span, format!("{name:?} takes 1 argument")));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                self.unify_at(span, st, Type::String)?;
                Ok((Type::String, se))
            }
            Builtin::StrSplit => {
                if args.len() != 2 {
                    return Err(at(span, "split takes 2 arguments"));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                let (ct, ce) = self.infer_expr(&args[1])?;
                self.unify_at(span, st, Type::String)?;
                self.unify_at(span, ct, Type::Char)?;
                Ok((Type::List(Box::new(Type::String)), self.union_eff(se, ce)))
            }
            Builtin::StrSubstring => {
                if args.len() != 3 {
                    return Err(at(span, "substring takes 3 arguments (string, start, end)"));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                let (a, ae) = self.infer_expr(&args[1])?;
                let (b, be) = self.infer_expr(&args[2])?;
                self.unify_at(span, st, Type::String)?;
                self.unify_at(span, a, Type::Int)?;
                self.unify_at(span, b, Type::Int)?;
                Ok((Type::String, self.union3_eff(se, ae, be)))
            }
            Builtin::StrStartsWith | Builtin::StrEndsWith => {
                if args.len() != 2 {
                    return Err(at(span, "startsWith/endsWith takes 2 arguments"));
                }
                let (st, se) = self.infer_expr(&args[0])?;
                let (pt, pe) = self.infer_expr(&args[1])?;
                self.unify_at(span, st, Type::String)?;
                self.unify_at(span, pt, Type::String)?;
                Ok((Type::Bool, self.union_eff(se, pe)))
            }
            Builtin::ReadStdin => {
                if !args.is_empty() {
                    return Err(at(span, "readStdin takes 0 arguments"));
                }
                Ok((Type::String, Effect::io()))
            }
            Builtin::MatchFail => {
                if !args.is_empty() {
                    return Err(at(span, "match fail takes 0 arguments"));
                }
                // Diverges; fresh var unifies with any arm result type.
                Ok((self.fresh(), Effect::pure()))
            }
            Builtin::Assert => {
                if args.len() != 1 {
                    return Err(at(span, "assert takes 1 argument"));
                }
                let (ct, ce) = self.infer_expr(&args[0])?;
                self.unify_at(span, ct, Type::Bool)?;
                Ok((Type::Unit, ce))
            }
        }
    }
}

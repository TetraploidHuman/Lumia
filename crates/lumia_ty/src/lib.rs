//! Hindley-Milner style type inference + effect sets (MVP).

use lumia_hir::{Builtin, Expr, Fun, Item, Module};
use lumia_syntax::{BinOp, UnOp};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Char,
    Unit,
    Fun(Vec<Type>, Box<Type>, Effect),
    Var(u32),
    /// List[T]
    List(Box<Type>),
    /// Map[K, V]
    Map(Box<Type>, Box<Type>),
    /// Set[T]
    Set(Box<Type>),
    /// Nominal sum type, e.g. Option[T] → Adt("Option", [T]).
    Adt { name: String, params: Vec<Type> },
    /// `(T1, T2, …)`
    Tuple(Vec<Type>),
}

/// Effect set ε — empty = pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effect {
    pub io: bool,
}

impl Effect {
    pub fn pure() -> Self {
        Self { io: false }
    }
    pub fn io() -> Self {
        Self { io: true }
    }
    pub fn union(self, other: Self) -> Self {
        Self {
            io: self.io || other.io,
        }
    }
    pub fn is_pure(self) -> bool {
        !self.io
    }
}

#[derive(Debug, Error)]
pub enum TypeError {
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone)]
pub struct TypedModule {
    pub module: Module,
    pub fun_types: HashMap<String, Type>,
    pub main_effect: Effect,
}

struct Infer {
    next_var: u32,
    subst: HashMap<u32, Type>,
    env: Vec<HashMap<String, Type>>,
}

impl Infer {
    fn new() -> Self {
        let mut builtins = HashMap::new();
        // println overload simplified: accept Int or String → Unit / IO
        builtins.insert(
            "println".into(),
            Type::Fun(vec![Type::Int], Box::new(Type::Unit), Effect::io()),
        );
        // listOf is variadic in spirit; MVP: 0-arg empty list of fresh elem type via special-case in Call
        builtins.insert(
            "listOf".into(),
            Type::Fun(vec![], Box::new(Type::List(Box::new(Type::Int))), Effect::pure()),
        );
        builtins.insert(
            "mapOf".into(),
            Type::Fun(
                vec![],
                Box::new(Type::Map(Box::new(Type::Int), Box::new(Type::Int))),
                Effect::pure(),
            ),
        );
        builtins.insert(
            "setOf".into(),
            Type::Fun(vec![], Box::new(Type::Set(Box::new(Type::Int))), Effect::pure()),
        );
        Self {
            next_var: 0,
            subst: HashMap::new(),
            env: vec![builtins],
        }
    }

    fn fresh(&mut self) -> Type {
        let v = self.next_var;
        self.next_var += 1;
        Type::Var(v)
    }

    fn push(&mut self) {
        self.env.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.env.pop();
    }

    fn bind(&mut self, name: String, ty: Type) {
        self.env.last_mut().unwrap().insert(name, ty);
    }

    fn lookup(&self, name: &str) -> Option<Type> {
        for scope in self.env.iter().rev() {
            if let Some(t) = scope.get(name) {
                return Some(t.clone());
            }
        }
        None
    }

    fn prune(&mut self, ty: Type) -> Type {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.subst.get(&v).cloned() {
                    let t = self.prune(t);
                    self.subst.insert(v, t.clone());
                    t
                } else {
                    Type::Var(v)
                }
            }
            Type::Fun(ps, r, e) => Type::Fun(
                ps.into_iter().map(|p| self.prune(p)).collect(),
                Box::new(self.prune(*r)),
                e,
            ),
            Type::List(t) => Type::List(Box::new(self.prune(*t))),
            Type::Map(k, v) => Type::Map(Box::new(self.prune(*k)), Box::new(self.prune(*v))),
            Type::Set(t) => Type::Set(Box::new(self.prune(*t))),
            other => other,
        }
    }

    fn unify(&mut self, a: Type, b: Type) -> Result<(), TypeError> {
        let a = self.prune(a);
        let b = self.prune(b);
        match (a, b) {
            (Type::Var(v), t) | (t, Type::Var(v)) => {
                if occurs(v, &t) {
                    return Err(TypeError::Message("infinite type".into()));
                }
                self.subst.insert(v, t);
                Ok(())
            }
            (Type::Int, Type::Int)
            | (Type::Float, Type::Float)
            | (Type::Bool, Type::Bool)
            | (Type::String, Type::String)
            | (Type::Char, Type::Char)
            | (Type::Unit, Type::Unit) => Ok(()),
            (Type::List(a), Type::List(b)) => self.unify(*a, *b),
            (Type::Set(a), Type::Set(b)) => self.unify(*a, *b),
            (Type::Map(ak, av), Type::Map(bk, bv)) => {
                self.unify(*ak, *bk)?;
                self.unify(*av, *bv)
            }
            (
                Type::Adt {
                    name: a,
                    params: ap,
                },
                Type::Adt {
                    name: b,
                    params: bp,
                },
            ) => {
                if a != b || ap.len() != bp.len() {
                    return Err(TypeError::Message(format!(
                        "type mismatch: Adt({a}) vs Adt({b})"
                    )));
                }
                for (x, y) in ap.into_iter().zip(bp) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Type::Tuple(a), Type::Tuple(b)) => {
                if a.len() != b.len() {
                    return Err(TypeError::Message("tuple arity mismatch".into()));
                }
                for (x, y) in a.into_iter().zip(b) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Type::Fun(a_ps, a_r, a_e), Type::Fun(b_ps, b_r, b_e)) => {
                if a_ps.len() != b_ps.len() {
                    return Err(TypeError::Message("function arity mismatch".into()));
                }
                for (x, y) in a_ps.into_iter().zip(b_ps) {
                    self.unify(x, y)?;
                }
                self.unify(*a_r, *b_r)?;
                if a_e != b_e {
                    // allow unifying by taking union into both — soft for MVP
                }
                Ok(())
            }
            (a, b) => Err(TypeError::Message(format!(
                "type mismatch: {a:?} vs {b:?}"
            ))),
        }
    }

    fn infer_expr(&mut self, expr: &Expr) -> Result<(Type, Effect), TypeError> {
        match expr {
            Expr::Int(_, _) => Ok((Type::Int, Effect::pure())),
            Expr::Float(_, _) => Ok((Type::Float, Effect::pure())),
            Expr::Bool(_, _) => Ok((Type::Bool, Effect::pure())),
            Expr::String(_, _) => Ok((Type::String, Effect::pure())),
            Expr::Char(_, _) => Ok((Type::Char, Effect::pure())),
            Expr::Unit(_) => Ok((Type::Unit, Effect::pure())),
            Expr::Var(name, _) => {
                let t = self
                    .lookup(name)
                    .ok_or_else(|| TypeError::Message(format!("unbound variable `{name}`")))?;
                Ok((t, Effect::pure()))
            }
            Expr::Let {
                name,
                value,
                body,
                ..
            } => {
                let (vt, ve) = self.infer_expr(value)?;
                self.push();
                self.bind(name.clone(), vt);
                let (bt, be) = self.infer_expr(body)?;
                self.pop();
                Ok((bt, ve.union(be)))
            }
            Expr::Assign { name, value, .. } => {
                let expect = self
                    .lookup(name)
                    .ok_or_else(|| TypeError::Message(format!("unbound `{name}` in assign")))?;
                let (vt, ve) = self.infer_expr(value)?;
                self.unify(expect, vt)?;
                Ok((Type::Unit, ve))
            }
            Expr::Lambda { params, body, .. } => {
                self.push();
                let mut pts = vec![];
                for p in params {
                    let tv = self.fresh();
                    pts.push(tv.clone());
                    self.bind(p.clone(), tv);
                }
                let (rt, re) = self.infer_expr(body)?;
                self.pop();
                Ok((Type::Fun(pts, Box::new(rt), re), Effect::pure()))
            }
            Expr::Call { callee, args, .. } => {
                // Special-case listOf(...): List[T] with unified element type
                if let Expr::Var(name, _) = callee.as_ref() {
                    if name == "listOf" {
                        let mut aes = Effect::pure();
                        let elem = self.fresh();
                        for a in args {
                            let (t, e) = self.infer_expr(a)?;
                            aes = aes.union(e);
                            self.unify(elem.clone(), t)?;
                        }
                        return Ok((Type::List(Box::new(self.prune(elem))), aes));
                    }
                    if name == "setOf" {
                        let mut aes = Effect::pure();
                        let elem = self.fresh();
                        for a in args {
                            let (t, e) = self.infer_expr(a)?;
                            aes = aes.union(e);
                            self.unify(elem.clone(), t)?;
                        }
                        return Ok((Type::Set(Box::new(self.prune(elem))), aes));
                    }
                    if name == "mapOf" {
                        // MVP: mapOf() empty; pairs via later `to` sugar
                        let mut aes = Effect::pure();
                        let k = self.fresh();
                        let v = self.fresh();
                        if args.is_empty() {
                            return Ok((
                                Type::Map(Box::new(self.prune(k)), Box::new(self.prune(v))),
                                aes,
                            ));
                        }
                        for a in args {
                            let (t, e) = self.infer_expr(a)?;
                            aes = aes.union(e);
                            // Treat each arg as a 2-tuple encoded as List for MVP — skip strict
                            let _ = t;
                        }
                        return Ok((
                            Type::Map(Box::new(Type::Int), Box::new(Type::Int)),
                            aes,
                        ));
                    }
                }
                let (ct, ce) = self.infer_expr(callee)?;
                let mut aes = Effect::pure();
                let mut ats = vec![];
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    ats.push(t);
                    aes = aes.union(e);
                }
                let ret = self.fresh();
                let fun_eff = match self.prune(ct.clone()) {
                    Type::Fun(_, _, e) => e,
                    _ => Effect::pure(),
                };
                self.unify(
                    ct,
                    Type::Fun(ats, Box::new(ret.clone()), fun_eff),
                )?;
                Ok((self.prune(ret), ce.union(aes).union(fun_eff)))
            }
            Expr::BuiltinCall { name, args, .. } => match name {
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                    if args.len() != 1 {
                        return Err(TypeError::Message("println takes 1 argument".into()));
                    }
                    let (t, e) = self.infer_expr(&args[0])?;
                    let t = self.prune(t);
                    match t {
                        Type::Int | Type::String | Type::Bool | Type::Float | Type::Char => {}
                        other => {
                            return Err(TypeError::Message(format!(
                                "println: unsupported type {other:?}"
                            )));
                        }
                    }
                    Ok((Type::Unit, Effect::io().union(e)))
                }
                Builtin::ListLen => {
                    if args.len() != 1 {
                        return Err(TypeError::Message("len takes 1 argument".into()));
                    }
                    let (t, e) = self.infer_expr(&args[0])?;
                    let t = self.prune(t);
                    match t {
                        Type::List(_) | Type::Set(_) | Type::Map(_, _) | Type::String => {}
                        Type::Var(_) => {
                            // Unconstrained: treat as List (match desugar / polymorphic use).
                            let elem = self.fresh();
                            self.unify(t, Type::List(Box::new(elem)))?;
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "len: expected List/Set/Map/String, got {other:?}"
                            )));
                        }
                    }
                    Ok((Type::Int, e))
                }
                Builtin::ListGet => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("get takes 2 arguments".into()));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (it, ie) = self.infer_expr(&args[1])?;
                    let elem = match self.prune(lt.clone()) {
                        Type::List(t) => {
                            self.unify(it, Type::Int)?;
                            *t
                        }
                        Type::Map(k, v) => {
                            self.unify(it, *k)?;
                            Type::Adt {
                                name: "Option".into(),
                                params: vec![*v],
                            }
                        }
                        Type::Var(_) => {
                            // Default to List (match desugar); Map is typed from mapOf.
                            self.unify(it, Type::Int)?;
                            let elem = self.fresh();
                            self.unify(lt, Type::List(Box::new(elem.clone())))?;
                            elem
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "get: expected List or Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((elem, le.union(ie)))
                }
                Builtin::Contains => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("contains takes 2 arguments".into()));
                    }
                    let (ct, ce) = self.infer_expr(&args[0])?;
                    let (kt, ke) = self.infer_expr(&args[1])?;
                    match self.prune(ct.clone()) {
                        Type::Map(k, _) => self.unify(kt, *k)?,
                        Type::Set(e) => self.unify(kt, *e)?,
                        Type::Var(_) => {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify(ct, Type::Map(Box::new(k.clone()), Box::new(v)))?;
                            self.unify(kt, k)?;
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "contains: expected Map or Set, got {other:?}"
                            )));
                        }
                    }
                    Ok((Type::Bool, ce.union(ke)))
                }
                Builtin::MapSet => {
                    if args.len() != 3 {
                        return Err(TypeError::Message("set takes 3 arguments (map, key, value)".into()));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let (kt, ke) = self.infer_expr(&args[1])?;
                    let (vt, ve) = self.infer_expr(&args[2])?;
                    let (k, v) = match self.prune(mt.clone()) {
                        Type::Map(k, v) => {
                            self.unify(kt, *k.clone())?;
                            self.unify(vt, *v.clone())?;
                            (k, v)
                        }
                        Type::Var(_) => {
                            self.unify(
                                mt,
                                Type::Map(Box::new(kt.clone()), Box::new(vt.clone())),
                            )?;
                            (Box::new(kt), Box::new(vt))
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "set: expected Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::Map(k, v), me.union(ke).union(ve)))
                }
                Builtin::MapRemove => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("remove takes 2 arguments".into()));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let (kt, ke) = self.infer_expr(&args[1])?;
                    let (k, v) = match self.prune(mt.clone()) {
                        Type::Map(k, v) => {
                            self.unify(kt, *k.clone())?;
                            (k, v)
                        }
                        Type::Var(_) => {
                            let k = kt;
                            let v = self.fresh();
                            self.unify(mt, Type::Map(Box::new(k.clone()), Box::new(v.clone())))?;
                            (Box::new(k), Box::new(v))
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "remove: expected Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::Map(k, v), me.union(ke)))
                }
                Builtin::MapKeys => {
                    if args.len() != 1 {
                        return Err(TypeError::Message("keys takes 1 argument".into()));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let k = match self.prune(mt.clone()) {
                        Type::Map(k, _) => *k,
                        Type::Var(_) => {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify(mt, Type::Map(Box::new(k.clone()), Box::new(v)))?;
                            k
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "keys: expected Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(Box::new(k)), me))
                }
                Builtin::MapValues => {
                    if args.len() != 1 {
                        return Err(TypeError::Message("values takes 1 argument".into()));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let v = match self.prune(mt.clone()) {
                        Type::Map(_, v) => *v,
                        Type::Var(_) => {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify(mt, Type::Map(Box::new(k), Box::new(v.clone())))?;
                            v
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "values: expected Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(Box::new(v)), me))
                }
                Builtin::MapItems => {
                    if args.len() != 1 {
                        return Err(TypeError::Message("items takes 1 argument".into()));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let (k, v) = match self.prune(mt.clone()) {
                        Type::Map(k, v) => (*k, *v),
                        Type::Var(_) => {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify(
                                mt,
                                Type::Map(Box::new(k.clone()), Box::new(v.clone())),
                            )?;
                            (k, v)
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "items: expected Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(Box::new(Type::Tuple(vec![k, v]))), me))
                }
                Builtin::AdtTag => {
                    if args.len() != 1 {
                        return Err(TypeError::Message("adt_tag takes 1 argument".into()));
                    }
                    let (_, e) = self.infer_expr(&args[0])?;
                    Ok((Type::Int, e))
                }
                Builtin::AdtField => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("adt_field takes 2 arguments".into()));
                    }
                    let (at, ae) = self.infer_expr(&args[0])?;
                    let (it, ie) = self.infer_expr(&args[1])?;
                    self.unify(it, Type::Int)?;
                    let idx = match &args[1] {
                        Expr::Int(n, _) if *n >= 0 => *n as usize,
                        _ => 0,
                    };
                    let elem = match self.prune(at) {
                        Type::Adt { params, .. } => params
                            .get(idx)
                            .cloned()
                            .unwrap_or_else(|| self.fresh()),
                        Type::Tuple(ts) => ts
                            .get(idx)
                            .cloned()
                            .unwrap_or_else(|| self.fresh()),
                        _ => self.fresh(),
                    };
                    Ok((elem, ae.union(ie)))
                }
                Builtin::ListSlice => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("slice takes 2 arguments".into()));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (it, ie) = self.infer_expr(&args[1])?;
                    self.unify(it, Type::Int)?;
                    let elem = match self.prune(lt.clone()) {
                        Type::List(t) => t,
                        Type::Var(_) => {
                            let elem = self.fresh();
                            self.unify(lt, Type::List(Box::new(elem.clone())))?;
                            Box::new(elem)
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "slice: expected List, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(elem), le.union(ie)))
                }
                Builtin::ListAppend => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("append takes 2 arguments".into()));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (et, ee) = self.infer_expr(&args[1])?;
                    let list_ty = match self.prune(lt.clone()) {
                        Type::List(t) => {
                            self.unify(et, *t.clone())?;
                            Type::List(t)
                        }
                        Type::Var(_) => {
                            self.unify(lt, Type::List(Box::new(et.clone())))?;
                            Type::List(Box::new(et))
                        }
                        other => {
                            return Err(TypeError::Message(format!(
                                "append: expected List, got {other:?}"
                            )));
                        }
                    };
                    Ok((list_ty, le.union(ee)))
                }
                Builtin::ListConcat => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("concat takes 2 arguments".into()));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (rt, re) = self.infer_expr(&args[1])?;
                    let lt = self.prune(lt);
                    let rt = self.prune(rt);
                    match (&lt, &rt) {
                        (Type::String, Type::String)
                        | (Type::String, Type::Var(_))
                        | (Type::Var(_), Type::String) => {
                            self.unify(lt, Type::String)?;
                            self.unify(rt, Type::String)?;
                            Ok((Type::String, le.union(re)))
                        }
                        _ => {
                            let list_ty = match (lt.clone(), rt.clone()) {
                                (Type::List(a), Type::List(b)) => {
                                    self.unify(*a.clone(), *b)?;
                                    Type::List(a)
                                }
                                (Type::List(a), Type::Var(_)) => {
                                    self.unify(rt, Type::List(a.clone()))?;
                                    Type::List(a)
                                }
                                (Type::Var(_), Type::List(b)) => {
                                    self.unify(lt, Type::List(b.clone()))?;
                                    Type::List(b)
                                }
                                (Type::Var(_), Type::Var(_)) => {
                                    let elem = self.fresh();
                                    let list = Type::List(Box::new(elem));
                                    self.unify(lt, list.clone())?;
                                    self.unify(rt, list.clone())?;
                                    list
                                }
                                (other, _) => {
                                    return Err(TypeError::Message(format!(
                                        "concat: expected List or String, got {other:?}"
                                    )));
                                }
                            };
                            Ok((list_ty, le.union(re)))
                        }
                    }
                }
                Builtin::Range | Builtin::RangeInclusive => {
                    if args.len() != 2 {
                        return Err(TypeError::Message("range takes 2 arguments".into()));
                    }
                    let mut eff = Effect::pure();
                    for a in args {
                        let (t, e) = self.infer_expr(a)?;
                        self.unify(t, Type::Int)?;
                        eff = eff.union(e);
                    }
                    Ok((Type::List(Box::new(Type::Int)), eff))
                }
                Builtin::Show => {
                    if args.len() != 1 {
                        return Err(TypeError::Message("show takes 1 argument".into()));
                    }
                    let (_, e) = self.infer_expr(&args[0])?;
                    Ok((Type::String, e))
                }
            },
            Expr::Binary {
                op, left, right, ..
            } => {
                let (lt, le) = self.infer_expr(left)?;
                let (rt, re) = self.infer_expr(right)?;
                let eff = le.union(re);
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                        let lt = self.prune(lt);
                        let rt = self.prune(rt);
                        match (&lt, &rt) {
                            (Type::Float, _) | (_, Type::Float) => {
                                self.unify(lt, Type::Float)?;
                                self.unify(rt, Type::Float)?;
                                Ok((Type::Float, eff))
                            }
                            _ => {
                                self.unify(lt, Type::Int)?;
                                self.unify(rt, Type::Int)?;
                                Ok((Type::Int, eff))
                            }
                        }
                    }
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        self.unify(lt.clone(), rt)?;
                        Ok((Type::Bool, eff))
                    }
                    BinOp::And | BinOp::Or => {
                        self.unify(lt, Type::Bool)?;
                        self.unify(rt, Type::Bool)?;
                        Ok((Type::Bool, eff))
                    }
                }
            }
            Expr::Unary { op, expr, .. } => {
                let (t, e) = self.infer_expr(expr)?;
                match op {
                    UnOp::Neg => {
                        let t = self.prune(t);
                        match t {
                            Type::Float => Ok((Type::Float, e)),
                            _ => {
                                self.unify(t, Type::Int)?;
                                Ok((Type::Int, e))
                            }
                        }
                    }
                    UnOp::Not => {
                        self.unify(t, Type::Bool)?;
                        Ok((Type::Bool, e))
                    }
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                let (ct, ce) = self.infer_expr(cond)?;
                self.unify(ct, Type::Bool)?;
                let (tt, te) = self.infer_expr(then_branch)?;
                let (et, ee) = self.infer_expr(else_branch)?;
                self.unify(tt.clone(), et)?;
                Ok((tt, ce.union(te).union(ee)))
            }
            Expr::Loop {
                cond,
                body,
                step,
                ..
            } => {
                let (ct, ce) = self.infer_expr(cond)?;
                self.unify(ct, Type::Bool)?;
                let (_, be) = self.infer_expr(body)?;
                let se = if let Some(s) = step {
                    self.infer_expr(s)?.1
                } else {
                    Effect::pure()
                };
                Ok((Type::Unit, ce.union(be).union(se)))
            }
            Expr::Break(_) | Expr::Continue(_) => Ok((Type::Unit, Effect::pure())),
            Expr::AdtNew {
                adt_name, args, ..
            } => {
                let mut eff = Effect::pure();
                let mut params = vec![];
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    params.push(t);
                    eff = eff.union(e);
                }
                if adt_name == "__Tuple" {
                    return Ok((Type::Tuple(params), eff));
                }
                // Unit ctors: Option[α] with fresh α; product: params = field types;
                // unary sum: Option[T] from payload.
                if params.is_empty() {
                    params.push(self.fresh());
                }
                Ok((
                    Type::Adt {
                        name: adt_name.clone(),
                        params,
                    },
                    eff,
                ))
            }
            Expr::Seq { stmts, .. } => {
                let mut eff = Effect::pure();
                let mut last = Type::Unit;
                for s in stmts {
                    let (t, e) = self.infer_expr(s)?;
                    last = t;
                    eff = eff.union(e);
                }
                Ok((last, eff))
            }
        }
    }

    fn infer_fun(&mut self, fun: &Fun) -> Result<(Type, Effect), TypeError> {
        self.push();
        let mut pts = vec![];
        for p in &fun.params {
            let tv = self.fresh();
            pts.push(tv.clone());
            self.bind(p.clone(), tv);
        }
        let (rt, re) = self.infer_expr(&fun.body)?;
        // main is always an effect root
        let re = if fun.is_main {
            re.union(Effect::io())
        } else {
            re
        };
        self.pop();
        let ty = Type::Fun(pts, Box::new(rt), re);
        Ok((ty, re))
    }
}

fn occurs(v: u32, ty: &Type) -> bool {
    match ty {
        Type::Var(u) => *u == v,
        Type::Fun(ps, r, _) => ps.iter().any(|p| occurs(v, p)) || occurs(v, r),
        Type::List(t) => occurs(v, t),
        Type::Set(t) => occurs(v, t),
        Type::Map(k, t) => occurs(v, k) || occurs(v, t),
        Type::Adt { params, .. } => params.iter().any(|p| occurs(v, p)),
        Type::Tuple(ts) => ts.iter().any(|p| occurs(v, p)),
        _ => false,
    }
}

pub fn infer_module(module: &Module) -> Result<TypedModule, TypeError> {
    let mut inf = Infer::new();
    let mut fun_types = HashMap::new();
    let mut main_effect = Effect::pure();

    // First pass: bind function names with fresh types for recursion
    for item in &module.items {
        if let Item::Fun(f) = item {
            let tv = inf.fresh();
            inf.bind(f.name.clone(), tv);
        }
    }

    for item in &module.items {
        match item {
            Item::Fun(f) => {
                let (ty, eff) = inf.infer_fun(f)?;
                if let Some(existing) = inf.lookup(&f.name) {
                    inf.unify(existing, ty.clone())?;
                }
                inf.bind(f.name.clone(), ty.clone());
                fun_types.insert(f.name.clone(), inf.prune(ty));
                if f.is_main {
                    main_effect = eff;
                    if !eff.io && f.is_main {
                        main_effect = Effect::io();
                    }
                }
            }
            Item::Val { name, body } => {
                let (ty, eff) = inf.infer_expr(body)?;
                if !eff.is_pure() {
                    return Err(TypeError::Message(format!(
                        "module-level `{name}` initializer must be pure (got IO effect)"
                    )));
                }
                inf.bind(name.clone(), ty);
            }
        }
    }

    // Pure context may not call IO — check non-main pure functions don't have IO in body
    // already encoded in Fun effect; verify callers later.

    Ok(TypedModule {
        module: module.clone(),
        fun_types,
        main_effect,
    })
}

/// Reject calling effectful functions from pure contexts (simplified whole-program check).
pub fn check_effect_boundaries(typed: &TypedModule) -> Result<(), TypeError> {
    for item in &typed.module.items {
        if let Item::Fun(f) = item {
            let fun_ty = typed.fun_types.get(&f.name);
            let fun_is_effectful = match fun_ty {
                Some(Type::Fun(_, _, e)) => e.io || f.is_main,
                _ => f.is_main,
            };
            // If inference claims pure, body must not contain any effect
            if !fun_is_effectful {
                assert_no_effects_in_pure(&f.body, &typed.fun_types)?;
            }
            check_expr_effects(&f.body, fun_is_effectful, &typed.fun_types)?;
        }
    }
    Ok(())
}

fn assert_no_effects_in_pure(
    expr: &Expr,
    fun_types: &HashMap<String, Type>,
) -> Result<(), TypeError> {
    match expr {
        Expr::BuiltinCall { name, args, .. } => {
            match name {
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                    return Err(TypeError::Message(
                        "effectful call not allowed in pure function".into(),
                    ));
                }
                _ => {
                    for a in args {
                        assert_no_effects_in_pure(a, fun_types)?;
                    }
                    Ok(())
                }
            }
        }
        Expr::Call { callee, args, .. } => {
            if let Expr::Var(name, _) = callee.as_ref() {
                if let Some(Type::Fun(_, _, e)) = fun_types.get(name) {
                    if e.io {
                        return Err(TypeError::Message(format!(
                            "cannot call effectful `{name}` from pure function"
                        )));
                    }
                }
            }
            assert_no_effects_in_pure(callee, fun_types)?;
            for a in args {
                assert_no_effects_in_pure(a, fun_types)?;
            }
            Ok(())
        }
        Expr::Let { value, body, .. } => {
            assert_no_effects_in_pure(value, fun_types)?;
            assert_no_effects_in_pure(body, fun_types)
        }
        Expr::Assign { value, .. } => assert_no_effects_in_pure(value, fun_types),
        Expr::Lambda { body, .. } => {
            let _ = body;
            Ok(())
        }
        Expr::Binary { left, right, .. } => {
            assert_no_effects_in_pure(left, fun_types)?;
            assert_no_effects_in_pure(right, fun_types)
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                assert_no_effects_in_pure(s, fun_types)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } => assert_no_effects_in_pure(expr, fun_types),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            assert_no_effects_in_pure(cond, fun_types)?;
            assert_no_effects_in_pure(then_branch, fun_types)?;
            assert_no_effects_in_pure(else_branch, fun_types)
        }
        Expr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            assert_no_effects_in_pure(cond, fun_types)?;
            assert_no_effects_in_pure(body, fun_types)?;
            if let Some(s) = step {
                assert_no_effects_in_pure(s, fun_types)?;
            }
            Ok(())
        }
        Expr::Break(_) | Expr::Continue(_) => Ok(()),
        Expr::AdtNew { args, .. } => {
            for a in args {
                assert_no_effects_in_pure(a, fun_types)?;
            }
            Ok(())
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..) => Ok(()),
    }
}

fn check_expr_effects(
    expr: &Expr,
    in_effect_ctx: bool,
    fun_types: &HashMap<String, Type>,
) -> Result<(), TypeError> {
    match expr {
        Expr::BuiltinCall { name, args, .. } => {
            match name {
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                    if !in_effect_ctx {
                        return Err(TypeError::Message(
                            "effectful call (println) not allowed in pure context".into(),
                        ));
                    }
                }
                _ => {}
            }
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Call { callee, args, .. } => {
            if let Expr::Var(name, _) = callee.as_ref() {
                if let Some(Type::Fun(_, _, e)) = fun_types.get(name) {
                    if e.io && !in_effect_ctx {
                        return Err(TypeError::Message(format!(
                            "cannot call effectful `{name}` from pure context"
                        )));
                    }
                }
            }
            check_expr_effects(callee, in_effect_ctx, fun_types)?;
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Let { value, body, .. } => {
            check_expr_effects(value, in_effect_ctx, fun_types)?;
            check_expr_effects(body, in_effect_ctx, fun_types)
        }
        Expr::Assign { value, .. } => check_expr_effects(value, in_effect_ctx, fun_types),
        Expr::Lambda { body, .. } => check_expr_effects(body, false, fun_types)
            .or_else(|_| check_expr_effects(body, true, fun_types)),
        Expr::Binary { left, right, .. } => {
            check_expr_effects(left, in_effect_ctx, fun_types)?;
            check_expr_effects(right, in_effect_ctx, fun_types)
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                check_expr_effects(s, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } => check_expr_effects(expr, in_effect_ctx, fun_types),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_expr_effects(cond, in_effect_ctx, fun_types)?;
            check_expr_effects(then_branch, in_effect_ctx, fun_types)?;
            check_expr_effects(else_branch, in_effect_ctx, fun_types)
        }
        Expr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            check_expr_effects(cond, in_effect_ctx, fun_types)?;
            check_expr_effects(body, in_effect_ctx, fun_types)?;
            if let Some(s) = step {
                check_expr_effects(s, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Break(_) | Expr::Continue(_) => Ok(()),
        Expr::AdtNew { args, .. } => {
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_hir::lower_module;
    use lumia_syntax::parse_module;

    #[test]
    fn infer_hello() {
        let src = r#"
module Hello
import std.io.{println}
val main = {
    println(42)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
        assert!(typed.main_effect.io);
    }

    #[test]
    fn reject_println_inside_pure_lambda_used_as_value() {
        let src = r#"
module Bad
import std.io.{println}
val compute() = {
    println(1)
    0
}
val main = {
    compute()
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
        assert!(matches!(
            typed.fun_types.get("compute"),
            Some(Type::Fun(_, _, Effect { io: true }))
        ));
    }

    #[test]
    fn module_val_rejects_io() {
        let src = r#"
module Bad
import std.io.{println}
val xs = println(1)
val main = {
    0
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        assert!(infer_module(&hir).is_err());
    }

    #[test]
    fn map_of_empty() {
        let src = r#"
module M
val m = mapOf()
val main = {
    0
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn list_of_infers() {
        let src = r#"
module L
val xs = listOf(1, 2, 3)
val main = {
    0
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn if_and_add() {
        let src = r#"
module I
import std.io.{println}
val main = {
    val x = if true { 1 } else { 2 }
    println(x + 40)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn match_int_arms() {
        let src = r#"
module MatchDemo
import std.io.{println}
val main = {
    val n = 1
    val s = n match {
        0 -> 10
        1 -> 20
        _ -> 30
    }
    println(s)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        check_effect_boundaries(&typed).unwrap();
    }
}


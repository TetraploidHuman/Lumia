use crate::ir::{CoreFun, Local};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashMap as HashMap;

/// Ground type key for monomorphization (Hash-friendly; no open Vars).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum MonoKind {
    Int,
    Float,
    Bool,
    String,
    Char,
    List(Box<MonoKind>),
    Map(Box<MonoKind>, Box<MonoKind>),
    Set(Box<MonoKind>),
    Adt {
        name: String,
        params: Vec<MonoKind>,
    },
    /// Named FunRef HOF argument (specialized + directized inside the clone).
    FunRef(String),
    /// Structural Fun (e.g. `Option[Fun(Float)→Float]` for unwrapOr).
    Fun {
        params: Vec<MonoKind>,
        ret: Box<MonoKind>,
    },
    /// `Task[T]` — needed so `unwrapTask(spawn { 1.5 })` clones Float ABI.
    Task(Box<MonoKind>),
    /// `Channel[T]` — same Float/heap payload specialization as Task.
    Channel(Box<MonoKind>),
    /// Structural tuple / tuple-prefix (HIR also uses `__Tuple` Adt; both must key).
    Tuple(Vec<MonoKind>),
    /// `()` — so `ignore(unit, 1.5)` can still specialize the Float result path.
    Unit,
}

impl MonoKind {
    fn encode(&self) -> String {
        match self {
            MonoKind::Int => "Int".into(),
            MonoKind::Float => "Float".into(),
            MonoKind::Bool => "Bool".into(),
            MonoKind::String => "String".into(),
            MonoKind::Char => "Char".into(),
            MonoKind::List(e) => format!("List_{}", e.encode()),
            MonoKind::Map(k, v) => format!("Map_{}_{}", k.encode(), v.encode()),
            MonoKind::Set(e) => format!("Set_{}", e.encode()),
            MonoKind::Adt { name, params } => {
                if params.is_empty() {
                    name.clone()
                } else {
                    format!(
                        "{}_{}",
                        name,
                        params
                            .iter()
                            .map(MonoKind::encode)
                            .collect::<Vec<_>>()
                            .join("_")
                    )
                }
            }
            MonoKind::FunRef(n) => {
                // Sanitize so `$` / path separators do not break the clone name.
                let safe: String = n
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                format!("Fn_{safe}")
            }
            MonoKind::Fun { params, ret } => {
                let ps: Vec<_> = params.iter().map(MonoKind::encode).collect();
                format!("Fun_{}_{}", ps.join("_"), ret.encode())
            }
            MonoKind::Task(e) => format!("Task_{}", e.encode()),
            MonoKind::Channel(e) => format!("Channel_{}", e.encode()),
            MonoKind::Tuple(es) => {
                let parts: Vec<_> = es.iter().map(MonoKind::encode).collect();
                format!("Tuple_{}", parts.join("_"))
            }
            MonoKind::Unit => "Unit".into(),
        }
    }

    pub(crate) fn to_type(&self) -> Type {
        match self {
            MonoKind::Int => Type::Int,
            MonoKind::Float => Type::Float,
            MonoKind::Bool => Type::Bool,
            MonoKind::String => Type::String,
            MonoKind::Char => Type::Char,
            MonoKind::List(e) => Type::List(Box::new(e.to_type())),
            MonoKind::Map(k, v) => Type::Map(Box::new(k.to_type()), Box::new(v.to_type())),
            MonoKind::Set(e) => Type::Set(Box::new(e.to_type())),
            MonoKind::Adt { name, params } => Type::Adt {
                name: name.clone(),
                params: params.iter().map(MonoKind::to_type).collect(),
            },
            // Opaque FunRef — never a fake `Fun([], Int)`. Resolve the named
            // function via `param_tys` / `ret_ty` (they take `&[CoreFun]`).
            MonoKind::FunRef(_) => Type::Unit,
            MonoKind::Fun { params, ret } => Type::Fun(
                params.iter().map(MonoKind::to_type).collect(),
                Box::new(ret.to_type()),
                Effect::pure(),
            ),
            MonoKind::Task(e) => Type::Task(Box::new(e.to_type())),
            MonoKind::Channel(e) => Type::Channel(Box::new(e.to_type())),
            MonoKind::Tuple(es) => Type::Tuple(es.iter().map(MonoKind::to_type).collect()),
            MonoKind::Unit => Type::Unit,
        }
    }
}

fn type_is_heap_structure(t: &Type) -> bool {
    // ABI-erased *containers* restored from Int keys — not the full GC lattice
    // ([`crate::type_may_heap`]): String/Char/Fun keep dedicated MonoKinds and
    // must not be rewritten from Int formals here.
    matches!(
        t,
        Type::Adt { .. }
            | Type::List(_)
            | Type::Map(_, _)
            | Type::Set(_)
            | Type::Task(_)
            | Type::Channel(_)
            | Type::Tuple(_)
            | Type::TuplePrefix(_)
    )
}

/// Build a [`MonoKey`] from concrete types (e.g. ListParMap callback ABI).
pub(crate) fn types_mono_key(tys: &[Type]) -> Option<MonoKey> {
    let mut kinds = Vec::with_capacity(tys.len());
    for t in tys {
        kinds.push(type_to_mono(t)?);
    }
    Some(MonoKey(kinds))
}

fn type_to_mono(t: &Type) -> Option<MonoKind> {
    match t {
        Type::Int => Some(MonoKind::Int),
        Type::Float => Some(MonoKind::Float),
        Type::Bool => Some(MonoKind::Bool),
        Type::String => Some(MonoKind::String),
        Type::Char => Some(MonoKind::Char),
        Type::List(e) => type_to_mono(e).map(|k| MonoKind::List(Box::new(k))),
        Type::Map(k, v) => Some(MonoKind::Map(
            Box::new(type_to_mono(k)?),
            Box::new(type_to_mono(v)?),
        )),
        Type::Set(e) => type_to_mono(e).map(|k| MonoKind::Set(Box::new(k))),
        Type::Adt { name, params } if lumia_hir::is_option_or_result(name) => {
            // Polymorphic payloads must appear in the key (map/andThen/…).
            // Open Vars are not ground — returning `Int` here created premature
            // `unwrapOr$Option_Int_Float` clones that blocked a later Float key
            // (call already rewritten to the Int clone).
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                if matches!(p, Type::Var(_)) {
                    return None;
                }
                ps.push(type_to_mono(p)?);
            }
            Some(MonoKind::Adt {
                name: name.clone(),
                params: ps,
            })
        }
        Type::Adt { name, params } => {
            // Keep call-site field kinds when present (`getx(Pt{x=1.5})` → Float
            // AdtField). Empty params: name-only key; `materialize_mono_param_tys`
            // restores the generic's structural formals (ABI-Int products).
            if params.is_empty() {
                return Some(MonoKind::Adt {
                    name: name.clone(),
                    params: vec![],
                });
            }
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                ps.push(type_to_mono(p)?);
            }
            Some(MonoKind::Adt {
                name: name.clone(),
                params: ps,
            })
        }
        // Structural Fun so `unwrapOr(Some(floatFun), …)` can specialize.
        // Open / unkeyable params or ret → whole key fails (same as List/Map/Adt),
        // never silently `unwrap_or(Int)` into a fake `$Fun_Int_…` clone.
        Type::Fun(ps, r, _) => {
            let mut pks = Vec::with_capacity(ps.len());
            for p in ps {
                pks.push(type_to_mono(p)?);
            }
            let rk = type_to_mono(r)?;
            Some(MonoKind::Fun {
                params: pks,
                ret: Box::new(rk),
            })
        }
        Type::Task(e) => type_to_mono(e).map(|k| MonoKind::Task(Box::new(k))),
        Type::Channel(e) => type_to_mono(e).map(|k| MonoKind::Channel(Box::new(k))),
        Type::Tuple(ts) | Type::TuplePrefix(ts) => {
            let mut ks = Vec::with_capacity(ts.len());
            for t in ts {
                ks.push(type_to_mono(t)?);
            }
            Some(MonoKind::Tuple(ks))
        }
        Type::Unit => Some(MonoKind::Unit),
        // Open Var: FunRef args use `MonoKind::FunRef` via funref map.
        _ => None,
    }
}

/// May-heap placeholder or open scalar — not a ground Option/Result payload.
fn is_erased_abi_ty(t: &Type) -> bool {
    match t {
        Type::Int | Type::Var(_) => true,
        Type::List(e) if matches!(e.as_ref(), Type::Int) => true,
        _ => false,
    }
}

fn is_erased_option_ret(t: &Type) -> bool {
    matches!(
        t,
        Type::Adt { name, params }
            if lumia_hir::is_option(name) && params.first().is_some_and(is_erased_abi_ty)
    )
}

fn is_erased_result_ret(t: &Type) -> bool {
    matches!(
        t,
        Type::Adt { name, params }
            if lumia_hir::is_result(name) && params.first().is_some_and(is_erased_abi_ty)
    )
}

pub(crate) fn strip_mono_suffix(name: &str) -> &str {
    name.split('$').next().unwrap_or(name)
}

/// Prefer [`CoreFun::mono_of`] / [`CoreFun::base_name`] when the callee is in `functions`.
fn base_fun_name<'a>(name: &'a str, functions: &'a [CoreFun]) -> &'a str {
    functions
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.base_name())
        .unwrap_or_else(|| strip_mono_suffix(name))
}

/// Clone rets must be re-keyable (`type_to_mono`); open Vars block `unwrapOr` etc.
pub(crate) fn ground_open_vars(t: Type) -> Type {
    match t {
        Type::Var(_) => Type::Int,
        Type::List(e) => Type::List(Box::new(ground_open_vars(*e))),
        Type::Set(e) => Type::Set(Box::new(ground_open_vars(*e))),
        Type::Task(e) => Type::Task(Box::new(ground_open_vars(*e))),
        Type::Channel(e) => Type::Channel(Box::new(ground_open_vars(*e))),
        Type::Map(k, v) => Type::Map(
            Box::new(ground_open_vars(*k)),
            Box::new(ground_open_vars(*v)),
        ),
        Type::Tuple(ts) => Type::Tuple(ts.into_iter().map(ground_open_vars).collect()),
        Type::TuplePrefix(ts) => Type::TuplePrefix(ts.into_iter().map(ground_open_vars).collect()),
        Type::Adt { name, params } => Type::Adt {
            name,
            params: params.into_iter().map(ground_open_vars).collect(),
        },
        Type::Fun(ps, r, e) => Type::Fun(
            ps.into_iter().map(ground_open_vars).collect(),
            Box::new(ground_open_vars(*r)),
            e,
        ),
        other => other,
    }
}

/// Restore container structure that mono keys intentionally collapse / that call
/// sites erase to ABI `Int`.
pub(crate) fn restore_mono_param_ty(key_ty: &mut Type, formal: Option<&Type>) {
    let Some(formal) = formal else {
        return;
    };
    match (&*key_ty, formal) {
        (Type::Int, t) if type_is_heap_structure(t) => {
            *key_ty = t.clone();
        }
        (
            Type::Adt { name, params },
            Type::Adt {
                name: formal_name,
                params: formal_params,
            },
        ) if name == formal_name && params.is_empty() && !formal_params.is_empty() => {
            *key_ty = formal.clone();
        }
        _ => {}
    }
}

/// `key.param_tys` then [`restore_mono_param_ty`] against the generic's formals.
pub(crate) fn materialize_mono_param_tys(
    key: &MonoKey,
    formals: &[Type],
    funs: &[CoreFun],
) -> Vec<Type> {
    let mut tys = key.param_tys(funs);
    for (i, ty) in tys.iter_mut().enumerate() {
        restore_mono_param_ty(ty, formals.get(i));
    }
    tys
}

/// Call-site specialization key: one ground kind per argument.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct MonoKey(pub(crate) Vec<MonoKind>);

impl MonoKey {
    /// Stable suffix: `$Float` / `$Bool` / `$String` when homogeneous; else `$List_Int` / `$Option_Float_Fn_dbl`.
    pub(crate) fn suffix(&self) -> String {
        let kinds = &self.0;
        if !kinds.is_empty()
            && kinds.iter().all(|k| matches!(k, MonoKind::Float))
            && !kinds.iter().any(|k| matches!(k, MonoKind::FunRef(_)))
        {
            return "$Float".into();
        }
        if !kinds.is_empty()
            && kinds.iter().all(|k| matches!(k, MonoKind::Bool))
            && !kinds.iter().any(|k| matches!(k, MonoKind::FunRef(_)))
        {
            return "$Bool".into();
        }
        if !kinds.is_empty()
            && kinds.iter().all(|k| matches!(k, MonoKind::String))
            && !kinds.iter().any(|k| matches!(k, MonoKind::FunRef(_)))
        {
            return "$String".into();
        }
        format!(
            "${}",
            kinds
                .iter()
                .map(MonoKind::encode)
                .collect::<Vec<_>>()
                .join("_")
        )
    }

    pub(crate) fn param_tys(&self, functions: &[CoreFun]) -> Vec<Type> {
        self.0
            .iter()
            .map(|k| match k {
                MonoKind::FunRef(n) => functions
                    .iter()
                    .find(|f| f.name == *n)
                    .map(|f| Type::Fun(f.param_tys.clone(), Box::new(f.ret_ty.clone()), f.effect))
                    // Missing name: Unit sentinel — not a fake 0-ary Fun.
                    .unwrap_or(Type::Unit),
                other => other.to_type(),
            })
            .collect()
    }

    /// Return type: HOF Option/Result map·andThen; else all-same / container /
    /// last-arg.
    ///
    /// `callee` (base name before `$…` suffix) distinguishes `mapErr` from
    /// `resultMap` — both are `Result + FunRef` keys with different Ok/Err flow.
    pub(crate) fn ret_ty(&self, functions: &[CoreFun], callee: Option<&str>) -> Type {
        if let Some(t) = self.hof_ret_ty(functions, callee) {
            return ground_open_vars(t);
        }
        let kinds = &self.0;
        if kinds.is_empty() {
            return Type::Int;
        }
        let base = callee
            .map(|c| base_fun_name(c, functions))
            .unwrap_or("");
        // `unwrapOr(opt, default)` returns the payload / default type — never the
        // Option/Result wrapper. When `default` is a FunRef, "last data arg" is
        // only the ADT and would wrongly keep `Option[Fun…]` (icall then uses
        // Int ABI on a Fun value).
        if base == "unwrapOr" {
            if let Some(MonoKind::Adt { name, params }) = kinds.first() {
                if lumia_hir::is_option_or_result(name) && !params.is_empty() {
                    let payload = ground_open_vars(params[0].to_type());
                    // FunRef default is the real ABI for `unwrapOr(None/Err, {…})`
                    // and must not be filtered out (that left Option_Int → Int icall).
                    let default_ty = match kinds.get(1) {
                        Some(MonoKind::FunRef(n)) => functions.iter().find(|f| f.name == *n).map(
                            |f| {
                                let mut params = f.param_tys.clone();
                                if f.is_lifted_lambda()
                                    && params
                                        .first()
                                        .is_some_and(|p| matches!(p, Type::Int | Type::Var(_)))
                                    && params.len() > 1
                                {
                                    params.remove(0);
                                }
                                Type::Fun(
                                    params,
                                    Box::new(f.ret_ty.clone()),
                                    f.effect,
                                )
                            },
                        ),
                        Some(k) => Some(ground_open_vars(k.to_type())),
                        None => None,
                    };
                    // Bare `None` / `Err("e")` often mistag the Ok/Some slot as
                    // Int or the Err String; the default carries the real ABI.
                    if let Some(def_ty) = default_ty {
                        let mistagged_payload = matches!(
                            payload,
                            Type::Int | Type::Var(_) | Type::String
                        );
                        let concrete_default = matches!(
                            def_ty,
                            Type::Float
                                | Type::Bool
                                | Type::Fun(_, _, _)
                                | Type::List(_)
                                | Type::Map(_, _)
                                | Type::Set(_)
                                | Type::Adt { .. }
                                | Type::Char
                        );
                        if mistagged_payload && concrete_default {
                            return def_ty;
                        }
                        if matches!(payload, Type::Int | Type::Var(_))
                            && !matches!(def_ty, Type::Int | Type::Var(_))
                        {
                            return def_ty;
                        }
                        // `unwrapOr(Some(fun), defaultFun)` — prefer Fun payload.
                        if matches!(payload, Type::Fun(_, _, _)) {
                            return payload;
                        }
                        if matches!(def_ty, Type::Fun(_, _, _))
                            && matches!(payload, Type::Int | Type::Var(_) | Type::String)
                        {
                            return def_ty;
                        }
                    }
                    return payload;
                }
            }
        }
        // Instantiate generic ret from key formals (after unwrapOr / HOF specials).
        // `unwrapTask: Task[a]→a` must not keep homogeneous `Task(Float)` as ret.
        if let Some(f) = functions.iter().find(|f| f.name == base) {
            if let Some(inst) = instantiate_ret_from_mono_key(f, self, functions) {
                return ground_open_vars(inst);
            }
        }
        if kinds.iter().all(|k| k == &kinds[0]) {
            if let MonoKind::FunRef(n) = &kinds[0] {
                return functions
                    .iter()
                    .find(|f| f.name == *n)
                    .map(|f| {
                        ground_open_vars(Type::Fun(
                            f.param_tys.clone(),
                            Box::new(f.ret_ty.clone()),
                            f.effect,
                        ))
                    })
                    .unwrap_or(Type::Unit);
            }
            return ground_open_vars(kinds[0].to_type());
        }
        // `l2Normalize(xs, eps)` / `keep(xs, eps)`: first List/Map/Set is the
        // value being transformed; last-arg would wrongly yield the scalar eps.
        // Only for the 2-arg shape — `sumAt(xs, i, acc)` is List+Int+Float and
        // must keep last-arg Float (fold/acc), not the list.
        let n_containers = kinds
            .iter()
            .filter(|k| {
                matches!(
                    k,
                    MonoKind::List(_) | MonoKind::Map(_, _) | MonoKind::Set(_)
                )
            })
            .count();
        if n_containers == 1 && kinds.len() == 2 {
            if let Some(k) = kinds.iter().find(|k| {
                matches!(
                    k,
                    MonoKind::List(_) | MonoKind::Map(_, _) | MonoKind::Set(_)
                )
            }) {
                return ground_open_vars(k.to_type());
            }
        }
        // Skip FunRef when taking "last data arg" (unwrap_or / defaults).
        ground_open_vars(
            kinds
                .iter()
                .rev()
                .find(|k| !matches!(k, MonoKind::FunRef(_)))
                .map(MonoKind::to_type)
                .unwrap_or(Type::Int),
        )
    }

    /// `map` / `andThen` / `mapErr` shaped keys with a FunRef callback.
    pub(crate) fn hof_ret_ty(&self, functions: &[CoreFun], callee: Option<&str>) -> Option<Type> {
        let base = callee
            .map(|c| base_fun_name(c, functions))
            .unwrap_or("");
        // `unwrapOr(opt, defaultFun)` also has Option+FunRef but FunRef is the
        // default value, not a mapper — must not wrap as `Option[…]`.
        if !matches!(
            base,
            "optionMap"
                | "resultMap"
                | "andThen"
                | "mapErr"
                | "map"
                | "flatMap"
                | "filterMap"
        ) {
            return None;
        }
        let fun_ret = self.0.iter().find_map(|k| match k {
            MonoKind::FunRef(n) => functions
                .iter()
                .find(|f| f.name == *n)
                .map(|f| f.ret_ty.clone()),
            _ => None,
        })?;
        // Shared Fun bodies often keep erased `Int` / may-heap `List(Int)` ret;
        // for Option/Result map, the data-arg payload is the best U then.
        // Also treat erased `Option[Var]` / `Result[Var,_]` as non-payload so
        // andThen does not wrap them into `Option[Option[Int]]`.
        let payload = match &fun_ret {
            t if is_erased_abi_ty(t) || is_erased_option_ret(t) || is_erased_result_ret(t) => None,
            other => Some(other.clone()),
        };
        let data = self.0.iter().find(|k| !matches!(k, MonoKind::FunRef(_)))?;
        match data {
            MonoKind::Adt { name, params } if lumia_hir::is_option(name) => {
                // `andThen` / `flatMap`: callback already returns `Option[U]`.
                // `map`: callback returns `U` → wrap as `Option[U]`.
                if matches!(&fun_ret, Type::Adt { name: n, .. } if lumia_hir::is_option(n))
                    && !is_erased_option_ret(&fun_ret)
                {
                    return Some(ground_open_vars(fun_ret));
                }
                let inner = payload
                    .filter(|t| !is_erased_abi_ty(t))
                    .or_else(|| params.first().map(MonoKind::to_type))?;
                Some(Type::Adt {
                    name: lumia_hir::OPTION.name.into(),
                    params: vec![ground_open_vars(inner)],
                })
            }
            MonoKind::Adt { name, params } if lumia_hir::is_result(name) => {
                // Always emit Result[Ok, Err] (two params). Callback rets often
                // carry only the Ok slot (`Result[Float]`); leaving an open Err
                // Var after refine blocks `type_to_mono` → no `unwrapOr$` clone.
                let data_err = params.get(1).map(MonoKind::to_type).unwrap_or(Type::Int);
                // `mapErr`: Ok stays data Ok; callback ret is the new Err.
                if base == "mapErr" {
                    let ok = params
                        .first()
                        .map(MonoKind::to_type)
                        .unwrap_or(Type::Int);
                    let err = payload
                        .filter(|t| !is_erased_abi_ty(t))
                        .or_else(|| params.get(1).map(MonoKind::to_type))
                        .unwrap_or(Type::Int);
                    return Some(Type::Adt {
                        name: lumia_hir::RESULT.name.into(),
                        params: vec![ground_open_vars(ok), ground_open_vars(err)],
                    });
                }
                if matches!(&fun_ret, Type::Adt { name: n, .. } if lumia_hir::is_result(n))
                    && !is_erased_result_ret(&fun_ret)
                {
                    let Type::Adt {
                        params: fun_ps, ..
                    } = &fun_ret
                    else {
                        unreachable!()
                    };
                    let ok = fun_ps.first().cloned().unwrap_or(Type::Int);
                    let err = fun_ps.get(1).cloned().unwrap_or(data_err);
                    return Some(Type::Adt {
                        name: lumia_hir::RESULT.name.into(),
                        params: vec![ground_open_vars(ok), ground_open_vars(err)],
                    });
                }
                let ok = payload
                    .filter(|t| !is_erased_abi_ty(t))
                    .or_else(|| params.first().map(MonoKind::to_type))?;
                Some(Type::Adt {
                    name: lumia_hir::RESULT.name.into(),
                    params: vec![ground_open_vars(ok), ground_open_vars(data_err)],
                })
            }
            _ => None,
        }
    }

    /// Clone when any arg is non-Int or a FunRef (HOF).
    pub(crate) fn worth_cloning(&self) -> bool {
        self.0.iter().any(|k| {
            matches!(k, MonoKind::FunRef(_) | MonoKind::Fun { .. })
                || !matches!(k, MonoKind::Int)
        })
    }

    pub(crate) fn funref_param_binds(&self, params: &[Local]) -> HashMap<u32, String> {
        let mut binds = HashMap::default();
        for (i, k) in self.0.iter().enumerate() {
            if let MonoKind::FunRef(n) = k {
                if let Some(p) = params.get(i) {
                    binds.insert(p.0, n.clone());
                }
            }
        }
        binds
    }
}

/// Bind type vars in `formal` from a ground `concrete` shape (mono key arg).
fn collect_mono_var_binds(formal: &Type, concrete: &Type, binds: &mut HashMap<u32, Type>) {
    match (formal, concrete) {
        (Type::Var(id), c) if !matches!(c, Type::Var(_)) => {
            binds.entry(*id).or_insert_with(|| c.clone());
        }
        (Type::List(fe), Type::List(ce))
        | (Type::Set(fe), Type::Set(ce))
        | (Type::Task(fe), Type::Task(ce))
        | (Type::Channel(fe), Type::Channel(ce)) => {
            collect_mono_var_binds(fe, ce, binds);
        }
        (Type::Map(fk, fv), Type::Map(ck, cv)) => {
            collect_mono_var_binds(fk, ck, binds);
            collect_mono_var_binds(fv, cv, binds);
        }
        (
            Type::Adt {
                name: n1,
                params: p1,
            },
            Type::Adt {
                name: n2,
                params: p2,
            },
        ) if n1 == n2 => {
            for (a, b) in p1.iter().zip(p2.iter()) {
                collect_mono_var_binds(a, b, binds);
            }
        }
        (Type::Fun(fps, fr, _), Type::Fun(cps, cr, _)) => {
            for (a, b) in fps.iter().zip(cps.iter()) {
                collect_mono_var_binds(a, b, binds);
            }
            collect_mono_var_binds(fr, cr, binds);
        }
        (Type::Tuple(fts), Type::Tuple(cts)) | (Type::TuplePrefix(fts), Type::TuplePrefix(cts)) => {
            for (a, b) in fts.iter().zip(cts.iter()) {
                collect_mono_var_binds(a, b, binds);
            }
        }
        _ => {}
    }
}

fn apply_mono_var_binds(t: &Type, binds: &HashMap<u32, Type>) -> Type {
    match t {
        Type::Var(id) => binds
            .get(id)
            .cloned()
            .unwrap_or_else(|| Type::Var(*id)),
        Type::List(e) => Type::List(Box::new(apply_mono_var_binds(e, binds))),
        Type::Set(e) => Type::Set(Box::new(apply_mono_var_binds(e, binds))),
        Type::Task(e) => Type::Task(Box::new(apply_mono_var_binds(e, binds))),
        Type::Channel(e) => Type::Channel(Box::new(apply_mono_var_binds(e, binds))),
        Type::Map(k, v) => Type::Map(
            Box::new(apply_mono_var_binds(k, binds)),
            Box::new(apply_mono_var_binds(v, binds)),
        ),
        Type::Adt { name, params } => Type::Adt {
            name: name.clone(),
            params: params
                .iter()
                .map(|p| apply_mono_var_binds(p, binds))
                .collect(),
        },
        Type::Fun(ps, r, e) => Type::Fun(
            ps.iter().map(|p| apply_mono_var_binds(p, binds)).collect(),
            Box::new(apply_mono_var_binds(r, binds)),
            *e,
        ),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|p| apply_mono_var_binds(p, binds)).collect()),
        Type::TuplePrefix(ts) => {
            Type::TuplePrefix(ts.iter().map(|p| apply_mono_var_binds(p, binds)).collect())
        }
        other => other.clone(),
    }
}

/// Instantiate `fun.ret_ty` from mono key arg shapes vs generic formals.
/// Returns `None` when the result still has open Vars (fall back to heuristics).
fn instantiate_ret_from_mono_key(
    fun: &CoreFun,
    key: &MonoKey,
    functions: &[CoreFun],
) -> Option<Type> {
    if fun.param_tys.len() != key.0.len() {
        return None;
    }
    // Skip when ret is already ground and not a Var-carrying shape we refine —
    // still allow Var / Task[Var] / etc.
    let mut binds = HashMap::default();
    let concretes = key.param_tys(functions);
    for (formal, concrete) in fun.param_tys.iter().zip(concretes.iter()) {
        collect_mono_var_binds(formal, concrete, &mut binds);
    }
    if binds.is_empty() {
        return None;
    }
    let inst = apply_mono_var_binds(&fun.ret_ty, &binds);
    if type_has_open_var(&inst) {
        return None;
    }
    Some(inst)
}

fn type_has_open_var(t: &Type) -> bool {
    match t {
        Type::Var(_) => true,
        Type::List(e) | Type::Set(e) | Type::Task(e) | Type::Channel(e) => type_has_open_var(e),
        Type::Map(k, v) => type_has_open_var(k) || type_has_open_var(v),
        Type::Adt { params, .. } => params.iter().any(type_has_open_var),
        Type::Fun(ps, r, _) => ps.iter().any(type_has_open_var) || type_has_open_var(r),
        Type::Tuple(ts) | Type::TuplePrefix(ts) => ts.iter().any(type_has_open_var),
        _ => false,
    }
}

/// Build a mono key from call-site arg types.
///
/// When `formals` is the callee's `param_tys` and a site arg is ABI-erased `Int`
/// but the formal is a heap structure (`Adt`/`List`/…), prefer the formal so the
/// key does not treat a product as a numeric `Int`.
pub(crate) fn args_mono_key(
    args: &[Local],
    local_tys: &HashMap<u32, Type>,
    funref_of: &HashMap<u32, String>,
    formals: Option<&[Type]>,
) -> Option<MonoKey> {
    let mut kinds = Vec::with_capacity(args.len());
    for (i, a) in args.iter().enumerate() {
        if let Some(name) = funref_of.get(&a.0) {
            kinds.push(MonoKind::FunRef(name.clone()));
            continue;
        }
        let mut ty = local_tys.get(&a.0)?.clone();
        if matches!(ty, Type::Int) {
            if let Some(formal) = formals.and_then(|f| f.get(i)) {
                if type_is_heap_structure(formal) {
                    // ABI-erased product: key by ADT name only so Int field
                    // guesses never enter the clone layout; materialize restores
                    // the generic formals. Call-site Adt{…, [Float,…]} keeps params.
                    ty = match formal {
                        Type::Adt { name, .. } if !lumia_hir::is_option_or_result(name) => {
                            Type::Adt {
                                name: name.clone(),
                                params: vec![],
                            }
                        }
                        other => other.clone(),
                    };
                }
            }
        }
        kinds.push(type_to_mono(&ty)?);
    }
    Some(MonoKey(kinds))
}

#[cfg(test)]
mod tests {
    use super::MonoKind;

    #[test]
    fn mono_kind_encode_stable_keys() {
        assert_eq!(MonoKind::Int.encode(), "Int");
        assert_eq!(
            MonoKind::List(Box::new(MonoKind::Float)).encode(),
            "List_Float"
        );
        assert_eq!(
            MonoKind::Map(Box::new(MonoKind::String), Box::new(MonoKind::Int)).encode(),
            "Map_String_Int"
        );
        assert_eq!(
            MonoKind::Adt {
                name: "Option".into(),
                params: vec![MonoKind::Float],
            }
            .encode(),
            "Option_Float"
        );
        assert_eq!(MonoKind::FunRef("a.b$c".into()).encode(), "Fn_a_b_c");
        assert_eq!(MonoKind::Unit.encode(), "Unit");
    }
}

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
        }
    }

    fn to_type(&self) -> Type {
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
            // Opaque Fun slot — clone body directizes to a named Call.
            MonoKind::FunRef(_) => Type::Fun(vec![], Box::new(Type::Int), Effect::pure()),
        }
    }
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
        Type::Adt { name, params } => {
            let mut ps = Vec::with_capacity(params.len());
            for p in params {
                ps.push(type_to_mono(p)?);
            }
            Some(MonoKind::Adt {
                name: name.clone(),
                params: ps,
            })
        }
        // Unit / Fun / Var: FunRef args use `MonoKind::FunRef` via funref map.
        _ => None,
    }
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
                    .unwrap_or_else(|| k.to_type()),
                other => other.to_type(),
            })
            .collect()
    }

    /// Return type: HOF Option/Result map·andThen; else all-same / last-arg.
    pub(crate) fn ret_ty(&self, functions: &[CoreFun]) -> Type {
        if let Some(t) = self.hof_ret_ty(functions) {
            return t;
        }
        let kinds = &self.0;
        if kinds.is_empty() {
            return Type::Int;
        }
        if kinds.iter().all(|k| k == &kinds[0]) {
            return kinds[0].to_type();
        }
        // Skip FunRef when taking "last data arg" (unwrap_or / defaults).
        kinds
            .iter()
            .rev()
            .find(|k| !matches!(k, MonoKind::FunRef(_)))
            .map(MonoKind::to_type)
            .unwrap_or(Type::Int)
    }

    /// `map` / `andThen` / `apply` shaped keys with a FunRef callback.
    pub(crate) fn hof_ret_ty(&self, functions: &[CoreFun]) -> Option<Type> {
        let (fun_name, fun_ret) = self.0.iter().find_map(|k| match k {
            MonoKind::FunRef(n) => {
                let f = functions.iter().find(|f| f.name == *n)?;
                Some((n.clone(), f.ret_ty.clone()))
            }
            _ => None,
        })?;
        let _ = fun_name;
        let first = self.0.first()?;
        // Shared Fun bodies often keep erased `Int` / heap-marker ret; for
        // Option/Result map, the payload kind is the best U when ret is erased.
        let refine_u = |u: Type, payload: &Type| -> Type {
            match (&u, payload) {
                (Type::Int | Type::Var(_), p) if !matches!(p, Type::Int | Type::Var(_)) => {
                    p.clone()
                }
                (Type::List(e), p) if matches!(e.as_ref(), Type::Int) => {
                    if !matches!(p, Type::Int | Type::Var(_)) {
                        p.clone()
                    } else {
                        u
                    }
                }
                _ => u,
            }
        };
        match first {
            MonoKind::Adt { name, params } if name == "Option" => {
                let payload = params.first().map(MonoKind::to_type).unwrap_or(Type::Int);
                match fun_ret {
                    // andThen: T → Option[U]
                    Type::Adt {
                        name,
                        params: mut ps,
                    } if name == "Option" => {
                        if let Some(u) = ps.first_mut() {
                            *u = refine_u(u.clone(), &payload);
                        }
                        Some(Type::Adt {
                            name: "Option".into(),
                            params: ps,
                        })
                    }
                    // map: T → U
                    other => Some(Type::Adt {
                        name: "Option".into(),
                        params: vec![refine_u(other, &payload)],
                    }),
                }
            }
            MonoKind::Adt { name, params } if name == "Result" => {
                let payload = params.first().map(MonoKind::to_type).unwrap_or(Type::Int);
                let e = params.get(1).map(MonoKind::to_type).unwrap_or(Type::Int);
                match fun_ret {
                    Type::Adt {
                        name,
                        params: mut ps,
                    } if name == "Result" => {
                        if let Some(u) = ps.first_mut() {
                            *u = refine_u(u.clone(), &payload);
                        }
                        Some(Type::Adt {
                            name: "Result".into(),
                            params: ps,
                        })
                    }
                    other => Some(Type::Adt {
                        name: "Result".into(),
                        params: vec![refine_u(other, &payload), e],
                    }),
                }
            }
            // apply(f, x) / similar: first slot is the FunRef.
            MonoKind::FunRef(_) => Some(fun_ret),
            _ => None,
        }
    }

    /// Int-only data sites stay shared; FunRef or non-Int ground → clone.
    pub(crate) fn worth_cloning(&self) -> bool {
        if self.0.is_empty() {
            return false;
        }
        self.0
            .iter()
            .any(|k| matches!(k, MonoKind::FunRef(_)) || !matches!(k, MonoKind::Int))
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

pub(crate) fn args_mono_key(
    args: &[Local],
    local_tys: &HashMap<u32, Type>,
    funref_of: &HashMap<u32, String>,
) -> Option<MonoKey> {
    let mut kinds = Vec::with_capacity(args.len());
    for a in args {
        if let Some(name) = funref_of.get(&a.0) {
            kinds.push(MonoKind::FunRef(name.clone()));
            continue;
        }
        let ty = local_tys.get(&a.0)?;
        kinds.push(type_to_mono(ty)?);
    }
    Some(MonoKey(kinds))
}

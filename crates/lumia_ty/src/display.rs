//! IDE-facing type pretty-printing (hover, inlay, completion detail).

use super::Type;
use rustc_hash::FxHashMap as HashMap;
use std::sync::Arc;

fn collect_vars(ty: &Type, out: &mut Vec<u32>) {
    match ty {
        Type::Var(v) => out.push(*v),
        Type::List(t) | Type::Set(t) | Type::Task(t) | Type::Channel(t) => collect_vars(t, out),
        Type::Map(k, v) => {
            collect_vars(k, out);
            collect_vars(v, out);
        }
        Type::Tuple(ts) | Type::TuplePrefix(ts) => {
            for t in ts {
                collect_vars(t, out);
            }
        }
        Type::Adt { params, .. } => {
            for p in params {
                collect_vars(p, out);
            }
        }
        Type::Fun(ps, r, _) => {
            for p in ps {
                collect_vars(p, out);
            }
            collect_vars(r, out);
        }
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Unit
        | Type::Unknown => {}
    }
}

/// Num MVP: arithmetic type vars default to Int in IDE display (DESIGN: numeric default Int).
pub fn subst_num_vars(ty: &Type, num_vars: &[u32]) -> Type {
    match ty {
        Type::Var(v) if num_vars.contains(v) => Type::Int,
        Type::Var(v) => Type::Var(*v),
        Type::List(t) => Type::List(Arc::new(subst_num_vars(t, num_vars))),
        Type::Set(t) => Type::Set(Arc::new(subst_num_vars(t, num_vars))),
        Type::Task(t) => Type::Task(Arc::new(subst_num_vars(t, num_vars))),
        Type::Channel(t) => Type::Channel(Arc::new(subst_num_vars(t, num_vars))),
        Type::Map(k, v) => Type::Map(
            Arc::new(subst_num_vars(k, num_vars)),
            Arc::new(subst_num_vars(v, num_vars)),
        ),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst_num_vars(t, num_vars)).collect()),
        Type::TuplePrefix(ts) => {
            Type::TuplePrefix(ts.iter().map(|t| subst_num_vars(t, num_vars)).collect())
        }
        Type::Adt { name, params } => Type::Adt {
            name: name.clone(),
            params: params.iter().map(|t| subst_num_vars(t, num_vars)).collect(),
        },
        Type::Fun(ps, r, e) => Type::Fun(
            ps.iter().map(|t| subst_num_vars(t, num_vars)).collect(),
            Arc::new(subst_num_vars(r, num_vars)),
            *e,
        ),
        other => other.clone(),
    }
}

/// Stable letter names for free type vars (`T`, `U`, …) instead of `?0`.
pub fn var_names_for(ty: &Type) -> HashMap<u32, String> {
    let mut vars = Vec::new();
    collect_vars(ty, &mut vars);
    vars.sort_unstable();
    vars.dedup();
    const LETTERS: &[&str] = &["T", "U", "V", "W", "X", "Y", "Z"];
    vars.iter()
        .enumerate()
        .map(|(i, v)| {
            let name = if i < LETTERS.len() {
                LETTERS[i].to_string()
            } else {
                format!("T{}", i - LETTERS.len() + 1)
            };
            (*v, name)
        })
        .collect()
}

/// Pretty-print with an explicit var→name map (shared across Fun params + return).
pub fn pretty_type_with(ty: &Type, names: &HashMap<u32, String>) -> String {
    match ty {
        Type::Var(v) => names.get(v).cloned().unwrap_or_else(|| format!("?{v}")),
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Char => "Char".into(),
        Type::Unit => "Unit".into(),
        Type::Unknown => "?".into(),
        Type::List(t) => format!("List[{}]", pretty_type_with(t, names)),
        Type::Set(t) => format!("Set[{}]", pretty_type_with(t, names)),
        Type::Task(t) => format!("Task[{}]", pretty_type_with(t, names)),
        Type::Channel(t) => format!("Channel[{}]", pretty_type_with(t, names)),
        Type::Map(k, v) => format!(
            "Map[{}, {}]",
            pretty_type_with(k, names),
            pretty_type_with(v, names)
        ),
        Type::Tuple(ts) => {
            let inner = ts
                .iter()
                .map(|t| pretty_type_with(t, names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Type::TuplePrefix(ts) => {
            let inner = ts
                .iter()
                .map(|t| pretty_type_with(t, names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner}, …)")
        }
        Type::Adt { name, params } => {
            if params.is_empty() {
                name.to_string()
            } else {
                let inner = params
                    .iter()
                    .map(|t| pretty_type_with(t, names))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}[{inner}]")
            }
        }
        Type::Fun(ps, r, e) => {
            let args = ps
                .iter()
                .map(|t| pretty_type_with(t, names))
                .collect::<Vec<_>>()
                .join(", ");
            let eff = if e.has_io() { " / IO" } else { "" };
            format!("({args}) -> {}{eff}", pretty_type_with(r, names))
        }
    }
}

/// IDE display: ground Num vars to Int, then name remaining vars `T`/`U`/….
pub fn display_type(ty: &Type, num_vars: &[u32]) -> String {
    let grounded = subst_num_vars(ty, num_vars);
    let names = var_names_for(&grounded);
    pretty_type_with(&grounded, &names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Effect;

    #[test]
    fn display_grounds_num_vars_and_names_rest() {
        let ty = Type::Fun(
            vec![Type::Var(0), Type::Var(1)],
            Arc::new(Type::Var(0)),
            Effect::pure(),
        );
        assert_eq!(display_type(&ty, &[0]), "(Int, T) -> Int");
        assert_eq!(display_type(&ty, &[]), "(T, U) -> T");
    }
}

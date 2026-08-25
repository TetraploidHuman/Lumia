//! IDE-facing type pretty-printing (hover, inlay, completion detail).

use super::Type;
use rustc_hash::FxHashMap as HashMap;

fn collect_vars(ty: &Type, out: &mut Vec<u32>) {
    ty.for_each(&mut |t| {
        if let Type::Var(v) = t {
            out.push(*v);
        }
    });
}

/// Num MVP: arithmetic type vars default to Int in IDE display (DESIGN: numeric default Int).
pub fn subst_num_vars(ty: &Type, num_vars: &[u32]) -> Type {
    ty.map(&mut |t| match t {
        Type::Var(v) if num_vars.contains(v) => Type::Int,
        other => other.clone(),
    })
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

fn fun_chain_has_io(ty: &Type) -> bool {
    match ty {
        Type::Fun(_, r, e) => e.has_io() || fun_chain_has_io(r),
        _ => false,
    }
}

fn pretty_type_with_effect(ty: &Type, names: &HashMap<u32, String>, emit_effect: bool) -> String {
    match ty {
        Type::Var(v) => names.get(v).cloned().unwrap_or_else(|| format!("?{v}")),
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Char => "Char".into(),
        Type::Unit => "Unit".into(),
        Type::List(t) => format!("List[{}]", pretty_type_with_effect(t, names, false)),
        Type::Set(t) => format!("Set[{}]", pretty_type_with_effect(t, names, false)),
        Type::Map(k, v) => format!(
            "Map[{}, {}]",
            pretty_type_with_effect(k, names, false),
            pretty_type_with_effect(v, names, false)
        ),
        Type::Tuple(ts) => {
            let inner = ts
                .iter()
                .map(|t| pretty_type_with_effect(t, names, false))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Type::TuplePrefix(ts) => {
            let inner = ts
                .iter()
                .map(|t| pretty_type_with_effect(t, names, false))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner}, …)")
        }
        Type::Adt { name, params } => {
            if params.is_empty() {
                name.clone()
            } else {
                let inner = params
                    .iter()
                    .map(|t| pretty_type_with_effect(t, names, false))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}[{inner}]")
            }
        }
        Type::Fun(ps, r, _) => {
            let ret = pretty_type_with_effect(r, names, false);
            let eff = if emit_effect && fun_chain_has_io(ty) {
                " / IO"
            } else {
                ""
            };
            let args_paren = if ps.is_empty() {
                "( )".into()
            } else {
                let args = ps
                    .iter()
                    .map(|t| pretty_type_with_effect(t, names, false))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({args})")
            };
            format!("{args_paren} -> {ret}{eff}")
        }
    }
}

/// Pretty-print with an explicit var→name map (shared across Fun params + return).
pub fn pretty_type_with(ty: &Type, names: &HashMap<u32, String>) -> String {
    pretty_type_with_effect(ty, names, true)
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
            Box::new(Type::Var(0)),
            Effect::pure(),
        );
        assert_eq!(display_type(&ty, &[0]), "(Int, T) -> Int");
        assert_eq!(display_type(&ty, &[]), "(T, U) -> T");
    }

    #[test]
    fn display_nested_io_effect_once() {
        let inner = Type::Fun(
            vec![Type::Var(0)],
            Box::new(Type::Unit),
            Effect::io(),
        );
        let outer = Type::Fun(vec![], Box::new(inner), Effect::io());
        assert_eq!(display_type(&outer, &[]), "( ) -> (T) -> Unit / IO");

        let inner_only = Type::Fun(vec![], Box::new(Type::Unit), Effect::io());
        assert_eq!(display_type(&inner_only, &[]), "( ) -> Unit / IO");
    }
}

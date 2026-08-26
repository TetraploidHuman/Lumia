//! Shared ADT branch-type joins (value inference + mono fixed-type refinement).

use lumi_ty::Type;

/// Prefer concrete over erased `Int` / open `Var` when merging one param slot.
pub(crate) fn merge_erased_param(a: &Type, b: &Type) -> Type {
    if a == b {
        return a.clone();
    }
    match (a, b) {
        (Type::Int, other) | (Type::Var(_), other) => other.clone(),
        (other, Type::Int) | (other, Type::Var(_)) => other.clone(),
        (l, r) if l == r => l.clone(),
        (l, _) => l.clone(),
    }
}

/// Join two ADT types with the same name (pad params, merge erased slots).
pub(crate) fn join_adt_types(a: &Type, b: &Type) -> Option<Type> {
    match (a, b) {
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
            if n1 == "Result" {
                Some(Type::Adt {
                    name: "Result".into(),
                    params: vec![
                        merge_erased_param(
                            p1.first().unwrap_or(&Type::Int),
                            p2.first().unwrap_or(&Type::Int),
                        ),
                        merge_erased_param(
                            p1.get(1).unwrap_or(&Type::Int),
                            p2.get(1).unwrap_or(&Type::Int),
                        ),
                    ],
                })
            } else if n1 == "Option" {
                Some(Type::Adt {
                    name: "Option".into(),
                    params: vec![merge_erased_param(
                        p1.first().unwrap_or(&Type::Int),
                        p2.first().unwrap_or(&Type::Int),
                    )],
                })
            } else {
                let n = p1.len().max(p2.len());
                let params = (0..n)
                    .map(|i| {
                        merge_erased_param(
                            p1.get(i).unwrap_or(&Type::Int),
                            p2.get(i).unwrap_or(&Type::Int),
                        )
                    })
                    .collect();
                Some(Type::Adt {
                    name: n1.clone(),
                    params,
                })
            }
        }
        _ => None,
    }
}

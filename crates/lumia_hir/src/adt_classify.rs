//! Shared sum-ADT field classification (ty infer + Core `sum_max_arity`).
//!
//! Marks which variant fields are recursive spines (`Nat.S`, `UList.Cons` tail)
//! vs parametric payloads. Keep a single algorithm — ty and lower both call here.

use crate::ast::AdtDef;
use rustc_hash::FxHashMap as HashMap;

/// Per-variant: `true` at field `i` ⇒ recursive spine (not a type parameter).
pub fn classify_sum_field_recursive(adt: &AdtDef) -> HashMap<lumia_syntax::Sym, Vec<bool>> {
    // Prelude Option/Result keep parametric payloads (Result is also special-cased
    // in ty `infer_adt_new`). Treating `Some` like `Nat.S` would require `Some(3): Option`.
    if crate::is_option_or_result(&adt.name) {
        return adt
            .variants
            .iter()
            .map(|v| (v.name.clone(), vec![false; v.arity]))
            .collect();
    }
    let arities: Vec<usize> = adt.variants.iter().map(|v| v.arity).collect();
    let has_nullary = arities.contains(&0);
    let only_nullary_unary = arities.iter().all(|&a| a <= 1);
    let mut out = HashMap::default();
    for v in &adt.variants {
        let rec = if v.arity == 0 {
            vec![]
        } else if only_nullary_unary && has_nullary {
            // `Nat { Z S(n) }`: the unary payload is `Nat` itself.
            vec![true; v.arity]
        } else if has_nullary && v.arity >= 2 {
            // `UList { Nil Cons(h, t) }`: last field recursive, earlier parametric.
            let mut k = vec![false; v.arity];
            if let Some(last) = k.last_mut() {
                *last = true;
            }
            k
        } else {
            // `Either` / `Shape` / `Expr`: all parametric (concatenated slots).
            vec![false; v.arity]
        };
        out.insert(v.name.clone(), rec);
    }
    out
}

/// Count type parameters for a sum ADT (= non-recursive payload fields).
pub fn sum_parametric_arity(adt: &AdtDef) -> usize {
    classify_sum_field_recursive(adt)
        .into_values()
        .map(|flags| flags.iter().filter(|&&r| !r).count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::AdtVariant;

    fn adt(name: &str, variants: &[(&str, usize)]) -> AdtDef {
        AdtDef {
            name: name.into(),
            variants: variants
                .iter()
                .enumerate()
                .map(|(i, (n, a))| AdtVariant {
                    name: (*n).into(),
                    tag: i as i64,
                    arity: *a,
                })
                .collect(),
        }
    }

    #[test]
    fn option_all_parametric() {
        let a = adt("Option", &[("None", 0), ("Some", 1)]);
        assert_eq!(sum_parametric_arity(&a), 1);
        let c = classify_sum_field_recursive(&a);
        assert_eq!(c["Some"], vec![false]);
    }

    #[test]
    fn nat_spine_not_parametric() {
        let a = adt("Nat", &[("Z", 0), ("S", 1)]);
        assert_eq!(sum_parametric_arity(&a), 0);
        assert_eq!(classify_sum_field_recursive(&a)["S"], vec![true]);
    }

    #[test]
    fn ulist_head_parametric_tail_spine() {
        let a = adt("UList", &[("Nil", 0), ("Cons", 2)]);
        assert_eq!(sum_parametric_arity(&a), 1);
        assert_eq!(classify_sum_field_recursive(&a)["Cons"], vec![false, true]);
    }
}

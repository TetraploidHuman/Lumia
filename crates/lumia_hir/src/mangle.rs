//! Stable name mangling shared by HIR lower and codegen.

/// Instance / default method: `__{Trait}_{Type}_{method}`.
///
/// Used by UFCS resolve, mono stubs, Show/Eq/Ord overrides, and trait dict registration.
#[inline]
pub fn mangle_trait_method(trait_name: &str, type_name: &str, method: &str) -> String {
    format!("__{trait_name}_{type_name}_{method}")
}

#[cfg(test)]
mod tests {
    use super::mangle_trait_method;

    #[test]
    fn mangle_matches_historical_format() {
        assert_eq!(
            mangle_trait_method("Show", "Point", "show"),
            "__Show_Point_show"
        );
        assert_eq!(mangle_trait_method("Eq", "Color", "eq"), "__Eq_Color_eq");
        assert_eq!(
            mangle_trait_method("Ord", "Point", "less"),
            "__Ord_Point_less"
        );
        assert_eq!(mangle_trait_method("Num", "Vec2", "add"), "__Num_Vec2_add");
    }
}

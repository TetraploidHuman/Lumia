//! Prelude ADT langitems — names and variant shapes shared by HIR lower / Core / ty / mono.

/// One variant of a prelude sum ADT (`Some` / `None`, …).
#[derive(Debug, Clone, Copy)]
pub struct PreludeVariant {
    pub name: &'static str,
    /// Number of payload fields (not counting the tag).
    pub arity: usize,
}

/// Named prelude ADT with a fixed variant list (tag = declaration index).
#[derive(Debug, Clone, Copy)]
pub struct PreludeAdt {
    pub name: &'static str,
    pub variants: &'static [PreludeVariant],
    /// HM type-parameter count (`Option` → 1, `Result` → 2).
    pub type_params: usize,
}

impl PreludeAdt {
    /// Tag assigned when the prelude injects this ADT (`variants` order).
    pub fn default_tag(&self, variant: &str) -> Option<i64> {
        self.variants
            .iter()
            .enumerate()
            .find(|(_, v)| v.name == variant)
            .map(|(i, _)| i as i64)
    }

    pub fn variant(&self, name: &str) -> Option<&'static PreludeVariant> {
        self.variants.iter().find(|v| v.name == name)
    }

    /// `(name, arity)` pairs for [`crate::lower`] prelude injection.
    pub fn variant_arities(&self) -> Vec<(&'static str, usize)> {
        self.variants.iter().map(|v| (v.name, v.arity)).collect()
    }
}

pub const OPTION: PreludeAdt = PreludeAdt {
    name: "Option",
    type_params: 1,
    variants: &[
        PreludeVariant {
            name: "Some",
            arity: 1,
        },
        PreludeVariant {
            name: "None",
            arity: 0,
        },
    ],
};

pub const RESULT: PreludeAdt = PreludeAdt {
    name: "Result",
    type_params: 2,
    variants: &[
        PreludeVariant {
            name: "Ok",
            arity: 1,
        },
        PreludeVariant {
            name: "Err",
            arity: 1,
        },
    ],
};

/// All compiler-injected prelude ADTs.
pub const PRELUDE_ADTS: &[PreludeAdt] = &[OPTION, RESULT];

/// Lookup by ADT name (`"Option"`, `"Result"`).
pub fn prelude_adt(name: &str) -> Option<&'static PreludeAdt> {
    PRELUDE_ADTS.iter().find(|a| a.name == name)
}

#[inline]
pub fn is_option(name: impl AsRef<str>) -> bool {
    name.as_ref() == OPTION.name
}

#[inline]
pub fn is_result(name: impl AsRef<str>) -> bool {
    name.as_ref() == RESULT.name
}

#[inline]
pub fn is_option_or_result(name: impl AsRef<str>) -> bool {
    let n = name.as_ref();
    is_option(n) || is_result(n)
}

/// HM type-parameter arity for a prelude sum, if any.
pub fn prelude_type_param_count(name: impl AsRef<str>) -> Option<usize> {
    prelude_adt(name.as_ref()).map(|a| a.type_params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_default_tags_match_injection_order() {
        assert_eq!(OPTION.default_tag("Some"), Some(0));
        assert_eq!(OPTION.default_tag("None"), Some(1));
        assert_eq!(RESULT.default_tag("Ok"), Some(0));
        assert_eq!(RESULT.default_tag("Err"), Some(1));
        assert_eq!(OPTION.type_params, 1);
        assert_eq!(RESULT.type_params, 2);
        assert!(is_option_or_result("Option"));
        assert!(is_option_or_result("Result"));
        assert!(!is_option_or_result("List"));
    }
}

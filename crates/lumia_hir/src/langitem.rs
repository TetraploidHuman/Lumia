//! Prelude ADT langitems — names and variant shapes shared by HIR lower / Core.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_default_tags_match_injection_order() {
        assert_eq!(OPTION.default_tag("Some"), Some(0));
        assert_eq!(OPTION.default_tag("None"), Some(1));
        assert_eq!(RESULT.default_tag("Ok"), Some(0));
        assert_eq!(RESULT.default_tag("Err"), Some(1));
    }
}

//! Boundaries where HIR still materializes owned `String`s (synthetic / mangled names).

use lumia_syntax::Sym;

/// Mangled / synthetic names not present in the source intern table.
#[inline]
pub(crate) fn synthetic(s: impl Into<String>) -> Sym {
    Sym::from(s.into())
}

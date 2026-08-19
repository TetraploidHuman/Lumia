//! Interned identifier spelling (`Arc<str>`). Cheap `Clone`, stable `Hash`/`Eq`.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

/// Spelling of an identifier or other small name in the syntax AST.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sym(Arc<str>);

impl Sym {
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[inline]
    pub fn arc(&self) -> &Arc<str> {
        &self.0
    }

    #[inline]
    pub(crate) fn from_arc(arc: Arc<str>) -> Self {
        Sym(arc)
    }
}

impl Deref for Sym {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Sym {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Enables `HashMap<Sym, V>::get("foo")` / `HashSet<Sym>::contains("foo")`.
impl Borrow<str> for Sym {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Sym {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Sym> for String {
    fn from(s: Sym) -> String {
        s.as_str().to_string()
    }
}

/// Test / synthetic AST helpers (not deduplicated).
impl From<&str> for Sym {
    fn from(s: &str) -> Self {
        Sym(Arc::from(s))
    }
}

impl From<String> for Sym {
    fn from(s: String) -> Self {
        Sym(Arc::from(s))
    }
}

impl PartialEq<str> for Sym {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Sym {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for Sym {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}

impl PartialEq<Sym> for String {
    fn eq(&self, other: &Sym) -> bool {
        self.as_str() == other.as_str()
    }
}

//! Per-module string interner for identifier and literal spellings.

use crate::sym::Sym;
use std::collections::HashMap;
use std::sync::Arc;

/// Dedupes identifier spellings while parsing one module.
#[derive(Debug, Default)]
pub struct StringInterner {
    index: HashMap<Arc<str>, ()>,
}

impl StringInterner {
    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(existing) = self.index.get_key_value(s) {
            return Sym::from_arc(existing.0.clone());
        }
        let arc: Arc<str> = Arc::from(s);
        self.index.insert(arc.clone(), ());
        Sym::from_arc(arc)
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupes_by_spelling() {
        let mut pool = StringInterner::default();
        let a = pool.intern("foo");
        let b = pool.intern("foo");
        let c = pool.intern("bar");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(Arc::ptr_eq(a.arc(), b.arc()));
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn dedupes_literal_spelling() {
        let mut pool = StringInterner::default();
        let a = pool.intern("hello");
        let b = pool.intern("hello");
        assert!(Arc::ptr_eq(a.arc(), b.arc()));
    }
}

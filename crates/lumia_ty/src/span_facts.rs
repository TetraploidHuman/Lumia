//! Span-keyed rewrite / fact tables.
//!
//! Several typing desugars (`ufcs` / `join` / `alt` / product field / `with`) key
//! facts by [`Span`]. Identical spans with **different** payloads would silently
//! overwrite and mis-rewrite the HIR — reject that as an ICE-style type error
//! until facts migrate to `NodeId`.

use crate::types::{at, TypeError};
use lumia_syntax::Span;
use rustc_hash::FxHashMap as HashMap;
use std::fmt::Debug;

/// Insert `val` at `span`, or error if a different value is already recorded.
pub(crate) fn insert_unique_span_fact<V: PartialEq + Debug>(
    map: &mut HashMap<Span, V>,
    span: Span,
    val: V,
    kind: &str,
) -> Result<(), TypeError> {
    match map.get(&span) {
        Some(old) if old != &val => Err(at(
            span,
            format!(
                "internal: conflicting {kind} rewrite at the same span \
                 ({old:?} vs {val:?}); report a compiler bug"
            ),
        )),
        _ => {
            map.insert(span, val);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_syntax::Span;

    #[test]
    fn allows_idempotent_insert() {
        let mut m = HashMap::default();
        let sp = Span::new(1, 2);
        insert_unique_span_fact(&mut m, sp, "a".to_string(), "test").unwrap();
        insert_unique_span_fact(&mut m, sp, "a".to_string(), "test").unwrap();
        assert_eq!(m.get(&sp).map(String::as_str), Some("a"));
    }

    #[test]
    fn rejects_conflicting_insert() {
        let mut m = HashMap::default();
        let sp = Span::new(3, 4);
        insert_unique_span_fact(&mut m, sp, "a".to_string(), "test").unwrap();
        let err = insert_unique_span_fact(&mut m, sp, "b".to_string(), "test").unwrap_err();
        assert!(
            err.message().contains("conflicting") && err.message().contains("test"),
            "{}",
            err.message()
        );
        assert_eq!(m.get(&sp).map(String::as_str), Some("a"));
    }
}

//! Fast name → function lookup for monomorphization.

use crate::ir::CoreFun;
use rustc_hash::FxHashMap;

/// Immutable name index into a function table (rebuilt when the table grows).
pub(crate) struct FunIndex<'a> {
    funs: &'a [CoreFun],
    by_name: FxHashMap<&'a str, usize>,
    pub(crate) sum_max_arity: &'a FxHashMap<String, usize>,
}

impl<'a> FunIndex<'a> {
    pub(crate) fn new(funs: &'a [CoreFun], sum_max_arity: &'a FxHashMap<String, usize>) -> Self {
        let mut by_name = FxHashMap::with_capacity_and_hasher(funs.len(), Default::default());
        for (i, f) in funs.iter().enumerate() {
            by_name.insert(f.name.as_str(), i);
        }
        Self {
            funs,
            by_name,
            sum_max_arity,
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&'a CoreFun> {
        self.by_name.get(name).map(|&i| &self.funs[i])
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub(crate) fn funs(&self) -> &'a [CoreFun] {
        self.funs
    }
}

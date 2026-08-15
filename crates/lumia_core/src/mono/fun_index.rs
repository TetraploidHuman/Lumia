//! Fast name → function lookup for monomorphization.

use crate::ir::CoreFun;
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

/// Immutable name index into a function table (rebuilt when the table grows).
pub(crate) struct FunIndex<'a> {
    funs: &'a [CoreFun],
    by_name: HashMap<&'a str, usize>,
    pub(crate) sum_max_arity: &'a HashMap<String, usize>,
    pub(crate) trait_methods: &'a HashMap<(String, String), Vec<String>>,
    pub(crate) channel_elem_hint: Option<&'a Type>,
}

impl<'a> FunIndex<'a> {
    pub(crate) fn new(
        funs: &'a [CoreFun],
        sum_max_arity: &'a HashMap<String, usize>,
        trait_methods: &'a HashMap<(String, String), Vec<String>>,
        channel_elem_hint: Option<&'a Type>,
    ) -> Self {
        let mut by_name = HashMap::with_capacity_and_hasher(funs.len(), Default::default());
        for (i, f) in funs.iter().enumerate() {
            by_name.insert(f.name.as_str(), i);
        }
        Self {
            funs,
            by_name,
            sum_max_arity,
            trait_methods,
            channel_elem_hint,
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

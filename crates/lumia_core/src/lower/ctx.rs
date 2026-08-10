//! Core lowering context (name → local bindings).

use crate::ir::Local;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) struct CoreLowerCtx {
    pub(super) next: u32,
    pub(super) name_to_local: HashMap<String, Local>,
    pub(super) mutables: HashSet<String>,
    pub(super) toplevel_funs: HashSet<String>,
    pub(super) toplevel_vals: HashSet<String>,
    /// Short trait-method names left unresolved until post-mono resolve.
    pub(super) trait_method_names: HashSet<String>,
}

impl CoreLowerCtx {
    pub(super) fn new(
        toplevel_funs: HashSet<String>,
        toplevel_vals: HashSet<String>,
        trait_method_names: HashSet<String>,
    ) -> Self {
        Self {
            next: 0,
            name_to_local: HashMap::default(),
            mutables: HashSet::default(),
            toplevel_funs,
            toplevel_vals,
            trait_method_names,
        }
    }

    pub(super) fn fresh(&mut self) -> Local {
        let l = Local(self.next);
        self.next += 1;
        l
    }

    pub(super) fn bind_name(&mut self, name: String, local: Local) {
        self.name_to_local.insert(name, local);
    }

    pub(super) fn bind_mutable(&mut self, name: String, local: Local) {
        self.mutables.insert(name.clone());
        self.bind_name(name, local);
    }

    /// Snapshot of name bindings (not `next` — locals stay unique across scopes).
    pub(super) fn save_bindings(&self) -> (HashMap<String, Local>, HashSet<String>) {
        (self.name_to_local.clone(), self.mutables.clone())
    }

    pub(super) fn restore_bindings(&mut self, saved: (HashMap<String, Local>, HashSet<String>)) {
        self.name_to_local = saved.0;
        self.mutables = saved.1;
    }
}

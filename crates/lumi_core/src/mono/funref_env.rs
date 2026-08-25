//! FunRef / slot-FunRef tracking shared by mono scan, rewrite, and directize.

use crate::{Local, Value};
use rustc_hash::FxHashMap as HashMap;

/// Propagates FunRef bindings through `Let` / `Assign` so HOF call sites see
/// the concrete callback name (and nested If/Loop inherit parent bindings).
pub(crate) struct FunRefEnv {
    pub funref_of: HashMap<u32, String>,
    pub slot_funrefs: HashMap<String, String>,
}

impl FunRefEnv {
    pub(crate) fn from_parents(
        parent_funrefs: &HashMap<u32, String>,
        parent_slot_funrefs: &HashMap<String, String>,
    ) -> Self {
        Self {
            funref_of: parent_funrefs.clone(),
            slot_funrefs: parent_slot_funrefs.clone(),
        }
    }

    pub(crate) fn note_let(&mut self, local: u32, value: &Value) {
        match value {
            Value::FunRef(name) => {
                self.funref_of.insert(local, name.clone());
            }
            Value::Local(Local(src)) => {
                if let Some(n) = self.funref_of.get(src).cloned() {
                    self.funref_of.insert(local, n);
                } else {
                    self.funref_of.remove(&local);
                }
            }
            Value::Name(n) => {
                if let Some(fr) = self.slot_funrefs.get(n).cloned() {
                    self.funref_of.insert(local, fr);
                } else {
                    self.funref_of.remove(&local);
                }
            }
            _ => {
                self.funref_of.remove(&local);
            }
        }
    }

    pub(crate) fn note_assign(&mut self, name: &str, value: Local) {
        if let Some(fr) = self.funref_of.get(&value.0).cloned() {
            self.slot_funrefs.insert(name.to_string(), fr);
        } else {
            self.slot_funrefs.remove(name);
        }
    }
}

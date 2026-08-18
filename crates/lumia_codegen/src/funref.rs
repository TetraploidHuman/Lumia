//! FunRef alias map shared by emit and capture-type analysis.

use lumia_core::{Local, Value};
use rustc_hash::FxHashMap as HashMap;

/// Whether binding an [`Value::AllocClosure`] should record a FunRef alias.
#[derive(Clone, Copy)]
pub(crate) enum AllocClosureFunref {
    /// Cap-ty walk: closure locals alias their lifted fun name.
    Track,
    /// Emit: only true [`Value::FunRef`] / Local chains (TCO / direct calls).
    Ignore,
}

/// Update `funref_locals` after `local = value`.
pub(crate) fn note_funref_local(
    funref_locals: &mut HashMap<u32, String>,
    local: u32,
    value: &Value,
    alloc_closure: AllocClosureFunref,
) {
    match value {
        Value::FunRef(name) => {
            funref_locals.insert(local, name.name.clone());
        }
        Value::AllocClosure { fun, .. } if matches!(alloc_closure, AllocClosureFunref::Track) => {
            funref_locals.insert(local, fun.name.clone());
        }
        Value::Local(Local(src)) => {
            if let Some(n) = funref_locals.get(src).cloned() {
                funref_locals.insert(local, n);
            } else {
                funref_locals.remove(&local);
            }
        }
        _ => {
            funref_locals.remove(&local);
        }
    }
}

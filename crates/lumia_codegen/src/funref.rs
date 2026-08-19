//! FunRef alias map shared by emit and capture-type analysis.
//!
//! Thin wrappers over [`lumia_core::FunRefAliases`] so codegen keeps the same
//! call sites while sharing the SSA + slot protocol with directize / lift / TCO.

use lumia_core::{FunRefAliases, FunRefAlloc, Local, Value};
use lumia_hir::Sym;

/// Whether binding an [`Value::AllocClosure`] should record a FunRef alias.
#[derive(Clone, Copy)]
pub(crate) enum AllocClosureFunref {
    /// Cap-ty walk: closure locals alias their lifted fun name.
    Track,
    /// Emit: only true [`Value::FunRef`] / Local chains (TCO / direct calls).
    Ignore,
}

impl From<AllocClosureFunref> for FunRefAlloc {
    fn from(v: AllocClosureFunref) -> Self {
        match v {
            AllocClosureFunref::Track => FunRefAlloc::Track,
            AllocClosureFunref::Ignore => FunRefAlloc::Ignore,
        }
    }
}

/// Update locals + named slots after a Let (emit / cap-ty share one env).
pub(crate) fn note_funref_let(
    aliases: &mut FunRefAliases,
    local: u32,
    value: &Value,
    alloc_closure: AllocClosureFunref,
) {
    aliases.note_let(local, value, alloc_closure.into(), None);
}

/// `var slot = local` — keep FunRef through named mut slots for TCO / IndirectCall.
pub(crate) fn note_funref_assign(aliases: &mut FunRefAliases, name: &Sym, value: Local) {
    aliases.note_assign(name.clone(), value);
}

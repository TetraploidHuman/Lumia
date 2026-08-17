//! ADT Show-kind registry — constructor names for recursive `lumia_show`.
//!
//! Heap ADTs pack a kind id into `type_id` bits `[31:16]` ([`lumia_abi::adt_type_id`]).
//! Kind `0` means anonymous (`#tag…`); kinds `≥ 1` index this table.
//!
//! # Safety (FFI)
//! `names` points to `n` NUL-terminated C strings (immortal).

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use crate::common::trap_abort;
use std::sync::Mutex;

struct AdtShowEntry {
    /// Immortal C string pointers (usually LLVM globals).
    ptrs: Vec<*const u8>,
}

// SAFETY: pointers are only read as NUL-terminated C strings; they outlive the process
// when registered from codegen globals / leaked owned strings.
unsafe impl Send for AdtShowEntry {}
unsafe impl Sync for AdtShowEntry {}

static ADT_SHOW: Mutex<Vec<Option<AdtShowEntry>>> = Mutex::new(Vec::new());

fn with_table<R>(f: impl FnOnce(&mut Vec<Option<AdtShowEntry>>) -> R) -> R {
    let mut guard = ADT_SHOW.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// Register variant labels for `kind` (`≥ 1`). `names` is `n` NUL-terminated strings by tag.
#[no_mangle]
pub unsafe extern "C" fn lumia_adt_register_show(kind: u32, names: *const *const u8, n: i64) {
    if kind == 0 || names.is_null() || n < 0 {
        trap_abort("lumia: adt_register_show invalid args");
    }
    let n = n as usize;
    let mut ptrs = Vec::with_capacity(n);
    unsafe {
        for i in 0..n {
            ptrs.push(*names.add(i));
        }
    }
    with_table(|table| {
        let idx = kind as usize;
        if table.len() <= idx {
            table.resize_with(idx + 1, || None);
        }
        table[idx] = Some(AdtShowEntry { ptrs });
    });
}

/// Clone registered name pointers for `kind` (empty if unset).
///
/// Returns an owned copy so callers can unlock before recursive Show (avoids Mutex deadlock).
pub(crate) fn adt_show_name_ptrs(kind: u16) -> Vec<*const u8> {
    if kind == 0 {
        return Vec::new();
    }
    with_table(|table| {
        let idx = kind as usize;
        table
            .get(idx)
            .and_then(|e| e.as_ref())
            .map(|e| e.ptrs.clone())
            .unwrap_or_default()
    })
}

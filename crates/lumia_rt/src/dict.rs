//! Runtime trait dictionaries (DESIGN: compiler-internal; users never write dict args).
//!
//! Instances register method pointers at startup; polymorphic / erased call sites
//! can look them up by `(trait, type_name)` when monomorphization did not bind a
//! direct mangled callee.

use crate::common::trap_abort;
use rustc_hash::FxHashMap;
use std::ffi::CStr;
use std::sync::Mutex;

/// Trait ids for dictionary registration (stable ABI; shared with codegen via `lumia_abi`).
pub use lumia_abi::{TRAIT_EQ, TRAIT_HASH, TRAIT_NUM, TRAIT_ORD, TRAIT_SHOW};

/// `trait_id → (type_name → method)`. Nested map lets lookup borrow `&str` without
/// allocating a `String` key on the hot path.
type DictTable = FxHashMap<i32, FxHashMap<Box<str>, usize>>;

/// Method pointers stored as `usize` so the table is `Send + Sync`.
static DICTS: Mutex<Option<DictTable>> = Mutex::new(None);

fn with_dicts<R>(f: impl FnOnce(&mut DictTable) -> R) -> R {
    let mut guard = DICTS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(FxHashMap::default);
    f(map)
}

fn cstr_name<'a>(name: *const u8) -> Option<&'a str> {
    if name.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(name as *const i8) }.to_str().ok()
}

/// Register a trait method implementation for `type_name` (nul-terminated).
#[no_mangle]
pub extern "C" fn lumia_dict_register(trait_id: i32, type_name: *const u8, method: *const ()) {
    let Some(name) = cstr_name(type_name) else {
        trap_abort("lumia: dict_register invalid args");
    };
    if name.is_empty() || method.is_null() {
        trap_abort("lumia: dict_register invalid args");
    }
    with_dicts(|m| {
        m.entry(trait_id)
            .or_default()
            .insert(Box::<str>::from(name), method as usize);
    });
}

/// Look up a registered method; returns null if missing.
#[no_mangle]
pub extern "C" fn lumia_dict_lookup(trait_id: i32, type_name: *const u8) -> *const () {
    let Some(name) = cstr_name(type_name) else {
        return std::ptr::null();
    };
    with_dicts(|m| {
        m.get(&trait_id)
            .and_then(|t| t.get(name))
            .copied()
            .map(|p| p as *const ())
            .unwrap_or(std::ptr::null())
    })
}

/// Show via dictionary: `method: fn(i64) -> *mut u8` (same ABI as mangled `__Show_*_show`).
#[no_mangle]
pub extern "C" fn lumia_dict_show(trait_id: i32, type_name: *const u8, value: i64) -> *mut u8 {
    let f = lumia_dict_lookup(trait_id, type_name);
    if f.is_null() {
        return std::ptr::null_mut();
    }
    let show: extern "C" fn(i64) -> *mut u8 = unsafe { std::mem::transmute(f) };
    show(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn show_stub(x: i64) -> *mut u8 {
        let _ = x;
        0xdead as *mut u8
    }

    #[test]
    fn register_and_lookup_show() {
        let name = b"UnitTestType\0";
        lumia_dict_register(TRAIT_SHOW, name.as_ptr(), show_stub as *const ());
        let p = lumia_dict_lookup(TRAIT_SHOW, name.as_ptr());
        assert!(!p.is_null());
        let out = lumia_dict_show(TRAIT_SHOW, name.as_ptr(), 0);
        assert_eq!(out, 0xdead as *mut u8);
    }
}

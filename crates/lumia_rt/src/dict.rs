//! Runtime trait dictionaries (DESIGN: compiler-internal; users never write dict args).
//!
//! Instances register method pointers at startup; polymorphic / erased call sites
//! can look them up by `(trait, type_name)` when monomorphization did not bind a
//! direct mangled callee.

use crate::common::trap_abort;
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Mutex;

/// Trait ids for dictionary registration (stable ABI).
pub const TRAIT_SHOW: i32 = 1;
pub const TRAIT_EQ: i32 = 2;
pub const TRAIT_ORD: i32 = 3;
pub const TRAIT_HASH: i32 = 4;
pub const TRAIT_NUM: i32 = 5;

type DictKey = (i32, String);

/// Method pointers stored as `usize` so the table is `Send + Sync`.
static DICTS: Mutex<Option<HashMap<DictKey, usize>>> = Mutex::new(None);

fn with_dicts<R>(f: impl FnOnce(&mut HashMap<DictKey, usize>) -> R) -> R {
    let mut guard = DICTS.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

fn cstr_name(name: *const u8) -> String {
    if name.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(name as *const i8) }
        .to_string_lossy()
        .into_owned()
}

/// Register a trait method implementation for `type_name` (nul-terminated).
#[no_mangle]
pub extern "C" fn lumia_dict_register(trait_id: i32, type_name: *const u8, method: *const ()) {
    let name = cstr_name(type_name);
    if name.is_empty() || method.is_null() {
        trap_abort("lumia: dict_register invalid args");
    }
    with_dicts(|m| {
        m.insert((trait_id, name), method as usize);
    });
}

/// Look up a registered method; returns null if missing.
#[no_mangle]
pub extern "C" fn lumia_dict_lookup(trait_id: i32, type_name: *const u8) -> *const () {
    let name = cstr_name(type_name);
    with_dicts(|m| {
        m.get(&(trait_id, name))
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
        let name = b"Point\0";
        lumia_dict_register(TRAIT_SHOW, name.as_ptr(), show_stub as *const ());
        let p = lumia_dict_lookup(TRAIT_SHOW, name.as_ptr());
        assert!(!p.is_null());
        let got = lumia_dict_show(TRAIT_SHOW, name.as_ptr(), 42);
        assert_eq!(got, 0xdead as *mut u8);
    }
}

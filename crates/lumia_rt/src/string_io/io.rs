//! Stdin / stdout helpers and auto println.
//!
//! # Safety (FFI)
//! `ptr`/`len` describe a valid byte buffer for println helpers.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use std::io::{self, Read, Write};

use super::string::lumia_alloc_string;
use crate::common::{
    header_from_payload, is_heap_payload, may_be_heap_payload_bits, tid_base, trap_abort, TYPE_ADT,
    TYPE_CHAR, TYPE_STRING,
};
use crate::show::lumia_show;
use lumia_abi::{is_list_tid, is_map_tid, is_set_tid};

#[no_mangle]
pub extern "C" fn lumia_println_int(n: i64) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{n}");
}

/// Soft cap so a hostile/huge stdin cannot force unbounded host allocation.
pub(crate) const MAX_STDIN_BYTES: usize = 64 * 1024 * 1024;

/// Read all of stdin into a heap String (UTF-8 bytes).
#[no_mangle]
pub extern "C" fn lumia_read_stdin() -> *mut u8 {
    let mut buf = Vec::new();
    let mut stdin = io::stdin().lock();
    let mut chunk = [0u8; 8192];
    loop {
        let n = match stdin.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => trap_abort(&format!("lumia: stdin read error: {e}")),
        };
        if buf.len().saturating_add(n) > MAX_STDIN_BYTES {
            trap_abort(&format!(
                "lumia: stdin exceeds {MAX_STDIN_BYTES} bytes (soft cap; use smaller input)"
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    unsafe { lumia_alloc_string(buf.as_ptr(), buf.len() as u64) }
}

///
/// # Safety
/// `ptr` is null or points to `len` readable bytes (not necessarily NUL-terminated).
#[no_mangle]
pub unsafe extern "C" fn lumia_println_str(ptr: *const u8, len: u64) {
    let mut out = io::stdout().lock();
    if ptr.is_null() {
        let _ = writeln!(out);
        return;
    }
    let slice = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let _ = out.write_all(slice);
    let _ = writeln!(out);
}

#[no_mangle]
pub extern "C" fn lumia_println_bool(b: i8) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{}", if b != 0 { "true" } else { "false" });
}

/// Print the Unit value (`Unit` + newline). Used by typed `println(())`.
#[no_mangle]
pub extern "C" fn lumia_println_unit() {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "Unit");
}

/// Print a NUL-terminated C string (from LLVM global string ptrs).
///
/// # Safety
/// `ptr` is null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lumia_println_cstr(ptr: *const u8) {
    let mut out = io::stdout().lock();
    if ptr.is_null() {
        let _ = writeln!(out);
        return;
    }
    unsafe {
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let _ = out.write_all(slice);
        let _ = writeln!(out);
    }
}

/// Print `x` as a heap String if it is one; ADTs via structural Show; otherwise Int.
#[no_mangle]
pub extern "C" fn lumia_println_auto(x: i64) {
    // Int/Bool/FunRef immediates cannot be managed payloads — skip heap Mutex.
    if !may_be_heap_payload_bits(x) {
        lumia_println_int(x);
        return;
    }
    let p = x as *mut u8;
    if is_heap_payload(p) {
        unsafe {
            let h = header_from_payload(p);
            if (*h).type_id == TYPE_STRING {
                let len = (*h).size as u64;
                lumia_println_str(p, len);
                return;
            }
            if (*h).type_id == TYPE_CHAR {
                let cp = *(p as *const i64) as u32;
                let mut out = io::stdout().lock();
                if let Some(ch) = char::from_u32(cp) {
                    let _ = writeln!(out, "{ch}");
                } else {
                    let _ = writeln!(out, "\u{FFFD}");
                }
                return;
            }
            let tid = (*h).type_id;
            if tid_base(tid) == TYPE_ADT || is_list_tid(tid) || is_map_tid(tid) || is_set_tid(tid) {
                let s = lumia_show(x);
                let len = (*header_from_payload(s)).size as u64;
                lumia_println_str(s, len);
                return;
            }
        }
    }
    lumia_println_int(x);
}

#[no_mangle]
pub extern "C" fn lumia_println_float(n: f64) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{n}");
}

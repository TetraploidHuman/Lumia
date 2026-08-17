//! Match / assert / arithmetic trap ABI.
//!
//! # Safety (FFI)
//! `msg` is null or `msg_len` UTF-8 bytes.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use crate::common::trap_abort;

#[no_mangle]
pub extern "C" fn lumia_match_fail() {
    trap_abort("lumia: non-exhaustive match");
}

/// Abort if `cond` is false (0). `msg` is a UTF-8 message (e.g. `path:line: assert failed`).
#[no_mangle]
pub unsafe extern "C" fn lumia_assert(cond: i64, msg: *const u8, msg_len: i64) {
    if cond == 0 {
        let text = if msg.is_null() || msg_len <= 0 {
            "lumia: assert failed".to_string()
        } else {
            let slice = unsafe { std::slice::from_raw_parts(msg, msg_len as usize) };
            match std::str::from_utf8(slice) {
                Ok(s) => format!("lumia: {s}"),
                Err(_) => "lumia: assert failed".to_string(),
            }
        };
        eprintln!("{text}");
        std::process::abort();
    }
}

#[no_mangle]
pub extern "C" fn lumia_trap_div0() {
    trap_abort("lumia: division by zero");
}

#[no_mangle]
pub extern "C" fn lumia_trap_overflow() {
    trap_abort("lumia: integer overflow");
}

//! Heap String representation and UTF-8 text ops.
//!
//! # Safety (FFI)
//! String payloads / C buffers must be valid for the callee.

#![deny(clippy::not_unsafe_ptr_arg_deref)]

use std::ptr;

use crate::common::{
    header_from_payload, is_heap_payload_bits, trap_abort, GcInhibitGuard, TYPE_BYTES, TYPE_CHAR,
    TYPE_LIST, TYPE_STRING,
};
use crate::gc::{list_payload_bytes, lumia_alloc};

///
/// # Safety
/// `s`/`prefix` are null or valid String payloads.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_starts_with(s: *mut u8, prefix: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(prefix, |p| if bytes.starts_with(p) { 1 } else { 0 })
    })
}

///
/// # Safety
/// `s`/`suffix` are null or valid String payloads.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_ends_with(s: *mut u8, suffix: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(suffix, |p| if bytes.ends_with(p) { 1 } else { 0 })
    })
}

/// Substring search (`haystack.contains(needle)`).
///
/// # Safety
/// `s`/`needle` are null or valid String payloads.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_contains(s: *mut u8, needle: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(needle, |n| {
            if n.is_empty() || bytes.windows(n.len()).any(|w| w == n) {
                1
            } else {
                0
            }
        })
    })
}

/// Allocate a GC-managed byte buffer (for strings etc.).
///
/// # Safety
/// `ptr` is null or points to `len` readable bytes; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_alloc_string(ptr: *const u8, len: u64) -> *mut u8 {
    let dest = lumia_alloc(len, TYPE_STRING);
    if !dest.is_null() && len > 0 {
        unsafe {
            ptr::copy_nonoverlapping(ptr, dest, len as usize);
        }
    }
    dest
}

/// NUL-terminated C string copy of a Lumia String (for `foreign` String arguments).
///
/// # Safety
/// `s` is null or a valid String payload; returned C string is immortal/process-owned or null.
#[no_mangle]
pub unsafe extern "C" fn lumia_string_cstr(s: *mut u8) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    if s.is_null() {
        let dest = lumia_alloc(1, TYPE_BYTES);
        unsafe {
            *dest = 0;
        }
        return dest;
    }
    unsafe {
        let n = (*header_from_payload(s)).size as usize;
        let bytes = std::slice::from_raw_parts(s, n);
        if bytes.contains(&0) {
            trap_abort("lumia: String with interior NUL cannot convert to C string");
        }
        let nbytes = (n as u64)
            .checked_add(1)
            .filter(|&b| b <= u32::MAX as u64)
            .unwrap_or_else(|| trap_abort("lumia: cstr buffer too large"));
        let dest = lumia_alloc(nbytes, TYPE_BYTES);
        ptr::copy_nonoverlapping(s, dest, n);
        *dest.add(n) = 0;
        dest
    }
}

/// Build a Lumia String from a NUL-terminated C string (foreign String returns).
///
/// # Safety
/// `cstr` is null or a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn lumia_cstr_to_string(cstr: *const u8) -> *mut u8 {
    if cstr.is_null() {
        return unsafe { lumia_alloc_string(std::ptr::null(), 0) };
    }
    unsafe {
        let mut n = 0usize;
        while *cstr.add(n) != 0 {
            n += 1;
            if n > 1 << 28 {
                trap_abort("lumia: cstr too long");
            }
        }
        lumia_alloc_string(cstr, n as u64)
    }
}

pub(crate) fn with_str_bytes<R>(s: *mut u8, f: impl FnOnce(&[u8]) -> R) -> R {
    if s.is_null() {
        return f(&[]);
    }
    unsafe {
        let n = (*header_from_payload(s)).size as usize;
        f(std::slice::from_raw_parts(s, n))
    }
}

/// Unicode scalar count (DESIGN: `String` is UTF-8 text). Invalid UTF-8 is
/// counted via lossy decoding (U+FFFD per bad sequence).
fn utf8_char_count(bytes: &[u8]) -> i64 {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.chars().count() as i64,
        Err(_) => String::from_utf8_lossy(bytes).chars().count() as i64,
    }
}

/// Byte length of the heap string payload (for C/`println` marshalling).
///
/// # Safety
/// `s` is null or a valid String payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_byte_len(s: *mut u8) -> i64 {
    if s.is_null() {
        return 0;
    }
    unsafe { (*header_from_payload(s)).size as i64 }
}

/// Codepoint length — user-facing `.len()` on `String`.
///
/// # Safety
/// `s` is null or a valid String payload.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_len(s: *mut u8) -> i64 {
    with_str_bytes(s, utf8_char_count)
}

///
/// # Safety
/// `a`/`b` are null or valid String payloads; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    // Keep `a`/`b` alive across the destination allocation.
    let _gc = GcInhibitGuard::enter();
    unsafe {
        let na = if a.is_null() {
            0u64
        } else {
            (*header_from_payload(a)).size as u64
        };
        let nb = if b.is_null() {
            0u64
        } else {
            (*header_from_payload(b)).size as u64
        };
        let total = na
            .checked_add(nb)
            .filter(|&t| t <= u32::MAX as u64)
            .unwrap_or_else(|| trap_abort("lumia: string too large to concat"));
        let dest = lumia_alloc(total, TYPE_STRING);
        if dest.is_null() {
            trap_abort("lumia: str concat OOM");
        }
        if na > 0 {
            ptr::copy_nonoverlapping(a, dest, na as usize);
        }
        if nb > 0 {
            ptr::copy_nonoverlapping(b, dest.add(na as usize), nb as usize);
        }
        dest
    }
}

pub(crate) fn char_codepoint(ch: i64) -> u32 {
    let p = ch as *mut u8;
    if is_heap_payload_bits(ch) {
        unsafe {
            if (*header_from_payload(p)).type_id == TYPE_CHAR {
                return *(p as *const i64) as u32;
            }
        }
    }
    ch as u32
}

/// Trim ASCII whitespace from both ends.
///
/// # Safety
/// `s` is null or a valid String payload; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_trim(s: *mut u8) -> *mut u8 {
    // Copy before alloc — slice aliases heap bytes that GC may free.
    with_str_bytes(s, |bytes| {
        let start = bytes
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(bytes.len());
        let end = bytes
            .iter()
            .rposition(|b| !b.is_ascii_whitespace())
            .map(|i| i + 1)
            .unwrap_or(start);
        let owned = bytes[start..end].to_vec();
        unsafe { lumia_alloc_string(owned.as_ptr(), owned.len() as u64) }
    })
}

/// Substring `[start, end)` in **Unicode scalar** offsets (clamped).
/// Never splits a multi-byte UTF-8 sequence.
///
/// # Safety
/// `s` is null or a valid String payload; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_substring(s: *mut u8, start: i64, end: i64) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let owned = str_substring_bytes(bytes, start, end);
        unsafe { lumia_alloc_string(owned.as_ptr(), owned.len() as u64) }
    })
}

fn str_substring_bytes(bytes: &[u8], start: i64, end: i64) -> Vec<u8> {
    match std::str::from_utf8(bytes) {
        Ok(text) => {
            let n = text.chars().count() as i64;
            let a = start.clamp(0, n) as usize;
            let b = end.clamp(0, n) as usize;
            let b = b.max(a);
            text.chars().skip(a).take(b - a).collect::<String>().into_bytes()
        }
        Err(_) => {
            let cow = String::from_utf8_lossy(bytes);
            let n = cow.chars().count() as i64;
            let a = start.clamp(0, n) as usize;
            let b = end.clamp(0, n) as usize;
            let b = b.max(a);
            cow.chars().skip(a).take(b - a).collect::<String>().into_bytes()
        }
    }
}

/// Prefix of `n` Unicode scalars (clamped), like List `.take`.
///
/// # Safety
/// `s` is null or a valid String payload; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_take(s: *mut u8, n: i64) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let end = if n < 0 { 0 } else { n };
        let owned = str_substring_bytes(bytes, 0, end);
        unsafe { lumia_alloc_string(owned.as_ptr(), owned.len() as u64) }
    })
}

/// Drop first `n` Unicode scalars (clamped), like List `.drop` / `slice`.
///
/// # Safety
/// `s` is null or a valid String payload; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_slice(s: *mut u8, n: i64) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let start = if n < 0 { 0 } else { n };
        let end = utf8_char_count(bytes);
        let owned = str_substring_bytes(bytes, start, end);
        unsafe { lumia_alloc_string(owned.as_ptr(), owned.len() as u64) }
    })
}

/// Reverse Unicode scalars.
///
/// # Safety
/// `s` is null or a valid String payload; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_reverse(s: *mut u8) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let owned = match std::str::from_utf8(bytes) {
            Ok(text) => text.chars().rev().collect::<String>().into_bytes(),
            Err(_) => String::from_utf8_lossy(bytes)
                .chars()
                .rev()
                .collect::<String>()
                .into_bytes(),
        };
        unsafe { lumia_alloc_string(owned.as_ptr(), owned.len() as u64) }
    })
}

///
/// # Safety
/// `s` is null or a valid String payload; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_to_lower(s: *mut u8) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let lower = match std::str::from_utf8(bytes) {
            Ok(text) => text.to_lowercase(),
            Err(_) => String::from_utf8_lossy(bytes).to_lowercase(),
        };
        unsafe { lumia_alloc_string(lower.as_ptr(), lower.len() as u64) }
    })
}

///
/// # Safety
/// `s` is null or a valid String payload; returned String is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_to_upper(s: *mut u8) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let upper = match std::str::from_utf8(bytes) {
            Ok(text) => text.to_uppercase(),
            Err(_) => String::from_utf8_lossy(bytes).to_uppercase(),
        };
        unsafe { lumia_alloc_string(upper.as_ptr(), upper.len() as u64) }
    })
}

/// Split `s` on separator Char (or raw codepoint). Returns List[String].
///
/// # Safety
/// `s`/`sep` are null or valid String payloads; returned List[String] is newly allocated.
#[no_mangle]
pub unsafe extern "C" fn lumia_str_split(s: *mut u8, sep_ch: i64) -> *mut u8 {
    let _gc = GcInhibitGuard::enter();
    let cp = char_codepoint(sep_ch);
    let mut sep_buf = [0u8; 4];
    let sep = match char::from_u32(cp) {
        Some(c) => c.encode_utf8(&mut sep_buf).as_bytes().to_vec(),
        None => vec![cp as u8],
    };
    with_str_bytes(s, |bytes| {
        let mut parts: Vec<*mut u8> = Vec::new();
        if sep.is_empty() {
            parts.push(unsafe { lumia_alloc_string(bytes.as_ptr(), bytes.len() as u64) });
        } else {
            let mut start = 0usize;
            let mut i = 0usize;
            while i + sep.len() <= bytes.len() {
                if &bytes[i..i + sep.len()] == sep.as_slice() {
                    let slice = &bytes[start..i];
                    parts.push(unsafe { lumia_alloc_string(slice.as_ptr(), slice.len() as u64) });
                    i += sep.len();
                    start = i;
                } else {
                    i += 1;
                }
            }
            let slice = &bytes[start..];
            parts.push(unsafe { lumia_alloc_string(slice.as_ptr(), slice.len() as u64) });
        }
        let n = parts.len() as i64;
        let dest = lumia_alloc(list_payload_bytes(n), TYPE_LIST);
        unsafe {
            let dst = dest as *mut i64;
            *dst = n;
            for (i, p) in parts.into_iter().enumerate() {
                *dst.add(1 + i) = p as i64;
            }
        }
        dest
    })
}

#[cfg(test)]
#[path = "string_tests.rs"]
mod utf8_api_tests;

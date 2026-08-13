//! String / IO / trap helpers.

use std::io::{self, Read, Write};
use std::ptr;

use crate::common::{
    header_from_payload, is_heap_payload, tid_base, trap_abort, GcInhibitGuard, TYPE_ADT,
    TYPE_BYTES, TYPE_CHAR, TYPE_LIST, TYPE_STRING,
};
use crate::gc::{list_payload_bytes, lumia_alloc};
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
    lumia_alloc_string(buf.as_ptr(), buf.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_str_starts_with(s: *mut u8, prefix: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(prefix, |p| if bytes.starts_with(p) { 1 } else { 0 })
    })
}

#[no_mangle]
pub extern "C" fn lumia_str_ends_with(s: *mut u8, suffix: *mut u8) -> i64 {
    with_str_bytes(s, |bytes| {
        with_str_bytes(suffix, |p| if bytes.ends_with(p) { 1 } else { 0 })
    })
}

/// Substring search (`haystack.contains(needle)`).
#[no_mangle]
pub extern "C" fn lumia_str_contains(s: *mut u8, needle: *mut u8) -> i64 {
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

#[no_mangle]
pub extern "C" fn lumia_println_str(ptr: *const u8, len: u64) {
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

/// Print a NUL-terminated C string (from LLVM global string ptrs).
#[no_mangle]
pub extern "C" fn lumia_println_cstr(ptr: *const u8) {
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

/// Allocate a GC-managed byte buffer (for strings etc.).
#[no_mangle]
pub extern "C" fn lumia_alloc_string(ptr: *const u8, len: u64) -> *mut u8 {
    let dest = lumia_alloc(len, TYPE_STRING);
    if !dest.is_null() && len > 0 {
        unsafe {
            ptr::copy_nonoverlapping(ptr, dest, len as usize);
        }
    }
    dest
}

/// NUL-terminated C string copy of a Lumia String (for `foreign` String arguments).
#[no_mangle]
pub extern "C" fn lumia_string_cstr(s: *mut u8) -> *mut u8 {
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
#[no_mangle]
pub extern "C" fn lumia_cstr_to_string(cstr: *const u8) -> *mut u8 {
    if cstr.is_null() {
        return lumia_alloc_string(std::ptr::null(), 0);
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
/// Print `x` as a heap String if it is one; ADTs via structural Show; otherwise Int.
#[no_mangle]
pub extern "C" fn lumia_println_auto(x: i64) {
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
#[no_mangle]
pub extern "C" fn lumia_match_fail() {
    trap_abort("lumia: non-exhaustive match");
}

/// Abort if `cond` is false (0). `msg` is a UTF-8 message (e.g. `path:line: assert failed`).
#[no_mangle]
pub extern "C" fn lumia_assert(cond: i64, msg: *const u8, msg_len: i64) {
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
pub extern "C" fn lumia_str_len(s: *mut u8) -> i64 {
    if s.is_null() {
        return 0;
    }
    unsafe { (*header_from_payload(s)).size as i64 }
}
#[no_mangle]
pub extern "C" fn lumia_str_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
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
pub(crate) fn with_str_bytes<R>(s: *mut u8, f: impl FnOnce(&[u8]) -> R) -> R {
    if s.is_null() {
        return f(&[]);
    }
    unsafe {
        let n = (*header_from_payload(s)).size as usize;
        f(std::slice::from_raw_parts(s, n))
    }
}

pub(crate) fn char_codepoint(ch: i64) -> u32 {
    let p = ch as *mut u8;
    if !p.is_null() && is_heap_payload(p) {
        unsafe {
            if (*header_from_payload(p)).type_id == TYPE_CHAR {
                return *(p as *const i64) as u32;
            }
        }
    }
    ch as u32
}

/// Trim ASCII whitespace from both ends.
#[no_mangle]
pub extern "C" fn lumia_str_trim(s: *mut u8) -> *mut u8 {
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
        lumia_alloc_string(owned.as_ptr(), owned.len() as u64)
    })
}

/// Substring `[start, end)` in byte offsets (clamped).
#[no_mangle]
pub extern "C" fn lumia_str_substring(s: *mut u8, start: i64, end: i64) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let n = bytes.len() as i64;
        let a = start.clamp(0, n) as usize;
        let b = end.clamp(0, n) as usize;
        let b = b.max(a);
        let owned = bytes[a..b].to_vec();
        lumia_alloc_string(owned.as_ptr(), owned.len() as u64)
    })
}

#[no_mangle]
pub extern "C" fn lumia_str_to_lower(s: *mut u8) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let lower: Vec<u8> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
        lumia_alloc_string(lower.as_ptr(), lower.len() as u64)
    })
}

#[no_mangle]
pub extern "C" fn lumia_str_to_upper(s: *mut u8) -> *mut u8 {
    with_str_bytes(s, |bytes| {
        let upper: Vec<u8> = bytes.iter().map(|b| b.to_ascii_uppercase()).collect();
        lumia_alloc_string(upper.as_ptr(), upper.len() as u64)
    })
}

/// Split `s` on separator Char (or raw codepoint). Returns List[String].
#[no_mangle]
pub extern "C" fn lumia_str_split(s: *mut u8, sep_ch: i64) -> *mut u8 {
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
            parts.push(lumia_alloc_string(bytes.as_ptr(), bytes.len() as u64));
        } else {
            let mut start = 0usize;
            let mut i = 0usize;
            while i + sep.len() <= bytes.len() {
                if &bytes[i..i + sep.len()] == sep.as_slice() {
                    let slice = &bytes[start..i];
                    parts.push(lumia_alloc_string(slice.as_ptr(), slice.len() as u64));
                    i += sep.len();
                    start = i;
                } else {
                    i += 1;
                }
            }
            let slice = &bytes[start..];
            parts.push(lumia_alloc_string(slice.as_ptr(), slice.len() as u64));
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
#[no_mangle]
pub extern "C" fn lumia_trap_div0() {
    trap_abort("lumia: division by zero");
}

#[no_mangle]
pub extern "C" fn lumia_trap_overflow() {
    trap_abort("lumia: integer overflow");
}

//! Lumia runtime: pluggable GC ABI + first MmBackend (STW mark-sweep).
//!
//! C ABI contract used by codegen:
//! - `lumia_alloc(nbytes, type_id) -> *mut u8`
//! - `lumia_root_push(*mut *mut u8)` / `lumia_root_pop()`
//! - `lumia_write_barrier(obj, field_index, new_ptr)` (no-op for now)
//! - `lumia_gc_collect()`
//! - `lumia_println_int(i64)` / `lumia_println_str(*const u8, len)`

use std::alloc::{alloc, dealloc, Layout};
use std::cell::RefCell;
use std::io::{self, Write};
use std::ptr;
use std::sync::Mutex;

/// Object header placed before payload.
#[repr(C)]
pub struct ObjectHeader {
    pub type_id: u32,
    pub size: u32,
    pub marked: u32,
    pub _pad: u32,
}

/// Type ids (descriptor table later).
pub const TYPE_BYTES: u32 = 1;
pub const TYPE_STRING: u32 = 2;
pub const TYPE_LIST: u32 = 3;
pub const TYPE_MAP: u32 = 4;
pub const TYPE_SET: u32 = 5;
pub const TYPE_ADT: u32 = 6;
pub const TYPE_CHAR: u32 = 7;
/// Heap closure: `[fn_ptr:i64][cap0:i64]…`
pub const TYPE_CLOSURE: u32 = 8;

thread_local! {
    static HEAP: RefCell<Vec<*mut ObjectHeader>> = const { RefCell::new(Vec::new()) };
    static ROOTS: RefCell<Vec<*mut *mut u8>> = const { RefCell::new(Vec::new()) };
    static BYTES_ALLOCATED: RefCell<usize> = const { RefCell::new(0) };
}

static HEAP_LIMIT: Mutex<usize> = Mutex::new(256 * 1024);

fn header_layout(payload: usize) -> Layout {
    let size = std::mem::size_of::<ObjectHeader>() + payload;
    Layout::from_size_align(size, 8).unwrap()
}

fn payload_ptr(header: *mut ObjectHeader) -> *mut u8 {
    unsafe { (header as *mut u8).add(std::mem::size_of::<ObjectHeader>()) }
}

fn header_from_payload(payload: *mut u8) -> *mut ObjectHeader {
    unsafe { payload.sub(std::mem::size_of::<ObjectHeader>()) as *mut ObjectHeader }
}

/// MmBackend trait — swap mark-sweep for semispace etc. behind same ABI.
pub trait MmBackend {
    fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8;
    fn collect(&mut self);
    fn write_barrier(&mut self, _obj: *mut u8, _field: u32, _new: *mut u8) {}
}

pub struct MarkSweep;

impl MarkSweep {
    fn mark_from_roots() {
        ROOTS.with(|r| {
            for root in r.borrow().iter() {
                unsafe {
                    let p = **root;
                    if !p.is_null() {
                        mark(header_from_payload(p));
                    }
                }
            }
        });
    }

    fn sweep() {
        HEAP.with(|h| {
            let mut heap = h.borrow_mut();
            let mut i = 0;
            while i < heap.len() {
                let obj = heap[i];
                unsafe {
                    if (*obj).marked == 0 {
                        let layout = header_layout((*obj).size as usize);
                        dealloc(obj as *mut u8, layout);
                        heap.swap_remove(i);
                        continue;
                    }
                    (*obj).marked = 0;
                }
                i += 1;
            }
        });
        BYTES_ALLOCATED.with(|b| *b.borrow_mut() = 0);
    }
}

fn mark(obj: *mut ObjectHeader) {
    unsafe {
        if obj.is_null() || (*obj).marked != 0 {
            return;
        }
        (*obj).marked = 1;
        // MVP: no pointer fields traced yet (scalars / opaque blobs)
        let _ = (*obj).type_id;
    }
}

impl MmBackend for MarkSweep {
    fn alloc(&mut self, nbytes: usize, type_id: u32) -> *mut u8 {
        let limit = *HEAP_LIMIT.lock().unwrap();
        let used = BYTES_ALLOCATED.with(|b| *b.borrow());
        if used + nbytes > limit {
            self.collect();
        }
        let layout = header_layout(nbytes);
        unsafe {
            let mem = alloc(layout);
            if mem.is_null() {
                self.collect();
                let mem = alloc(layout);
                if mem.is_null() {
                    panic!("lumia: out of memory");
                }
                return finish_alloc(mem, nbytes, type_id);
            }
            finish_alloc(mem, nbytes, type_id)
        }
    }

    fn collect(&mut self) {
        Self::mark_from_roots();
        Self::sweep();
    }
}

unsafe fn finish_alloc(mem: *mut u8, nbytes: usize, type_id: u32) -> *mut u8 {
    let header = mem as *mut ObjectHeader;
    (*header).type_id = type_id;
    (*header).size = nbytes as u32;
    (*header).marked = 0;
    (*header)._pad = 0;
    HEAP.with(|h| h.borrow_mut().push(header));
    BYTES_ALLOCATED.with(|b| *b.borrow_mut() += nbytes);
    payload_ptr(header)
}

thread_local! {
    static BACKEND: RefCell<MarkSweep> = const { RefCell::new(MarkSweep) };
}

#[no_mangle]
pub extern "C" fn lumia_alloc(nbytes: u64, type_id: u32) -> *mut u8 {
    BACKEND.with(|b| b.borrow_mut().alloc(nbytes as usize, type_id))
}

#[no_mangle]
pub extern "C" fn lumia_root_push(slot: *mut *mut u8) {
    ROOTS.with(|r| r.borrow_mut().push(slot));
}

#[no_mangle]
pub extern "C" fn lumia_root_pop() {
    ROOTS.with(|r| {
        let _ = r.borrow_mut().pop();
    });
}

#[no_mangle]
pub extern "C" fn lumia_write_barrier(obj: *mut u8, field: u32, new_ptr: *mut u8) {
    BACKEND.with(|b| b.borrow_mut().write_barrier(obj, field, new_ptr));
}

#[no_mangle]
pub extern "C" fn lumia_gc_collect() {
    BACKEND.with(|b| b.borrow_mut().collect());
}

#[no_mangle]
pub extern "C" fn lumia_println_int(n: i64) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{n}");
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

fn is_heap_payload(payload: *mut u8) -> bool {
    if payload.is_null() {
        return false;
    }
    let h = header_from_payload(payload);
    HEAP.with(|heap| heap.borrow().iter().any(|&p| p == h))
}

/// Print `x` as a heap String if it is one; otherwise as Int.
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
        }
    }
    lumia_println_int(x);
}

#[no_mangle]
pub extern "C" fn lumia_println_float(n: f64) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{n}");
}

/// Structural equality: heap Strings compare by bytes; otherwise i64 identity.
#[no_mangle]
pub extern "C" fn lumia_eq(a: i64, b: i64) -> i64 {
    let pa = a as *mut u8;
    let pb = b as *mut u8;
    if is_heap_payload(pa) && is_heap_payload(pb) {
        unsafe {
            let ha = header_from_payload(pa);
            let hb = header_from_payload(pb);
            if (*ha).type_id == TYPE_STRING && (*hb).type_id == TYPE_STRING {
                let na = (*ha).size as usize;
                let nb = (*hb).size as usize;
                if na != nb {
                    return 0;
                }
                let sa = std::slice::from_raw_parts(pa, na);
                let sb = std::slice::from_raw_parts(pb, nb);
                return if sa == sb { 1 } else { 0 };
            }
            if (*ha).type_id == TYPE_CHAR && (*hb).type_id == TYPE_CHAR {
                let ca = *(pa as *const i64);
                let cb = *(pb as *const i64);
                return if ca == cb { 1 } else { 0 };
            }
        }
    }
    if a == b {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn lumia_alloc_char(codepoint: i64) -> *mut u8 {
    let dest = lumia_alloc(8, TYPE_CHAR);
    if dest.is_null() {
        panic!("lumia: char OOM");
    }
    unsafe {
        *(dest as *mut i64) = codepoint;
    }
    dest
}

/// Format a value as a heap String (for interpolation).
/// Strings are returned as-is; Chars become one-character strings; otherwise decimal Int.
#[no_mangle]
pub extern "C" fn lumia_show(x: i64) -> *mut u8 {
    let p = x as *mut u8;
    if is_heap_payload(p) {
        unsafe {
            let h = header_from_payload(p);
            if (*h).type_id == TYPE_STRING {
                return p;
            }
            if (*h).type_id == TYPE_CHAR {
                let cp = *(p as *const i64) as u32;
                let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                return lumia_alloc_string(s.as_ptr(), s.len() as u64);
            }
        }
    }
    let s = x.to_string();
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_show_float(n: f64) -> *mut u8 {
    let s = n.to_string();
    lumia_alloc_string(s.as_ptr(), s.len() as u64)
}

#[no_mangle]
pub extern "C" fn lumia_str_len(s: *mut u8) -> i64 {
    if s.is_null() {
        return 0;
    }
    unsafe { (*header_from_payload(s)).size as i64 }
}

/// Byte-length / element-count for List, Map, Set, or String.
#[no_mangle]
pub extern "C" fn lumia_len(obj: *mut u8) -> i64 {
    if obj.is_null() {
        return 0;
    }
    unsafe {
        let h = header_from_payload(obj);
        match (*h).type_id {
            TYPE_STRING => (*h).size as i64,
            TYPE_LIST | TYPE_MAP | TYPE_SET => *(obj as *const i64),
            _ => panic!("lumia: len on unsupported type {}", (*h).type_id),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_str_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
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
        let dest = lumia_alloc(na + nb, TYPE_STRING);
        if dest.is_null() {
            panic!("lumia: str concat OOM");
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

/// Concatenate two Lists or two Strings (same type_id).
#[no_mangle]
pub extern "C" fn lumia_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    let ta = if a.is_null() {
        TYPE_LIST
    } else {
        unsafe { (*header_from_payload(a)).type_id }
    };
    let tb = if b.is_null() {
        ta
    } else {
        unsafe { (*header_from_payload(b)).type_id }
    };
    if ta == TYPE_STRING || tb == TYPE_STRING {
        if ta != TYPE_STRING || tb != TYPE_STRING {
            panic!("lumia: concat type mismatch");
        }
        return lumia_str_concat(a, b);
    }
    lumia_list_concat(a, b)
}

/// List payload layout: `[len:i64][elem0:i64]...` (matches codegen AllocList).
#[no_mangle]
pub extern "C" fn lumia_list_len(list: *mut u8) -> i64 {
    if list.is_null() {
        return 0;
    }
    unsafe { *(list as *const i64) }
}

#[no_mangle]
pub extern "C" fn lumia_list_get(list: *mut u8, index: i64) -> i64 {
    if list.is_null() || index < 0 {
        panic!("lumia: list get out of bounds");
    }
    unsafe {
        let len = *(list as *const i64);
        if index >= len {
            panic!("lumia: list get out of bounds");
        }
        let base = list as *const i64;
        *base.add(1 + index as usize)
    }
}

/// Return a new HeapList with `elem` appended.
#[no_mangle]
pub extern "C" fn lumia_list_append(list: *mut u8, elem: i64) -> *mut u8 {
    unsafe {
        let n = if list.is_null() {
            0i64
        } else {
            *(list as *const i64)
        };
        let nbytes = (1 + n as u64 + 1) * 8;
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        if dest.is_null() {
            panic!("lumia: list append OOM");
        }
        let dst = dest as *mut i64;
        *dst = n + 1;
        if !list.is_null() {
            let src = list as *const i64;
            for i in 0..n as usize {
                *dst.add(1 + i) = *src.add(1 + i);
            }
        }
        *dst.add(1 + n as usize) = elem;
        dest
    }
}

/// Return a new HeapList that is `a` followed by `b`.
#[no_mangle]
pub extern "C" fn lumia_list_concat(a: *mut u8, b: *mut u8) -> *mut u8 {
    unsafe {
        let na = if a.is_null() {
            0i64
        } else {
            *(a as *const i64)
        };
        let nb = if b.is_null() {
            0i64
        } else {
            *(b as *const i64)
        };
        let n = na + nb;
        let nbytes = (1 + n as u64) * 8;
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        if dest.is_null() {
            panic!("lumia: list concat OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        if !a.is_null() {
            let src = a as *const i64;
            for i in 0..na as usize {
                *dst.add(1 + i) = *src.add(1 + i);
            }
        }
        if !b.is_null() {
            let src = b as *const i64;
            for i in 0..nb as usize {
                *dst.add(1 + na as usize + i) = *src.add(1 + i);
            }
        }
        dest
    }
}

/// Return a new HeapList with elements from `start` to end.
#[no_mangle]
pub extern "C" fn lumia_list_slice(list: *mut u8, start: i64) -> *mut u8 {
    if list.is_null() {
        let dest = lumia_alloc(8, TYPE_LIST);
        unsafe {
            *(dest as *mut i64) = 0;
        }
        return dest;
    }
    unsafe {
        let len = *(list as *const i64);
        let start = if start < 0 { 0 } else { start };
        let n = if start >= len { 0 } else { (len - start) as u64 };
        let nbytes = (1 + n) * 8;
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        if dest.is_null() {
            panic!("lumia: slice OOM");
        }
        *(dest as *mut i64) = n as i64;
        let src = list as *const i64;
        let dst = dest as *mut i64;
        for i in 0..n {
            *dst.add(1 + i as usize) = *src.add(1 + start as usize + i as usize);
        }
        dest
    }
}

/// Build `[start, end)` as HeapList of i64.
#[no_mangle]
pub extern "C" fn lumia_range(start: i64, end: i64) -> *mut u8 {
    let n = if end > start { (end - start) as u64 } else { 0 };
    let nbytes = (1 + n) * 8;
    let dest = lumia_alloc(nbytes, TYPE_LIST);
    unsafe {
        *(dest as *mut i64) = n as i64;
        let base = dest as *mut i64;
        for i in 0..n {
            *base.add(1 + i as usize) = start + i as i64;
        }
    }
    dest
}

/// Build `[start, end]` inclusive.
#[no_mangle]
pub extern "C" fn lumia_range_inclusive(start: i64, end: i64) -> *mut u8 {
    if end < start {
        return lumia_range(start, start);
    }
    lumia_range(start, end + 1)
}

/// Map payload: `[n_pairs:i64][k0][v0]...` (insertion order; last key wins on set).
unsafe fn map_find(map: *mut u8, key: i64) -> Option<usize> {
    if map.is_null() {
        return None;
    }
    let n = *(map as *const i64);
    let base = map as *const i64;
    let mut found = None;
    for i in 0..n as usize {
        if *base.add(1 + i * 2) == key {
            found = Some(i);
        }
    }
    found
}

#[no_mangle]
pub extern "C" fn lumia_map_contains(map: *mut u8, key: i64) -> i64 {
    unsafe { if map_find(map, key).is_some() { 1 } else { 0 } }
}

/// Missing key → None ADT; hit → Some(value). Tags come from the program's `Option` decl.
#[no_mangle]
pub extern "C" fn lumia_map_get(
    map: *mut u8,
    key: i64,
    some_tag: i64,
    none_tag: i64,
) -> *mut u8 {
    unsafe {
        match map_find(map, key) {
            Some(i) => {
                let base = map as *const i64;
                let val = *base.add(2 + i * 2);
                alloc_adt(some_tag, &[val])
            }
            None => alloc_adt(none_tag, &[]),
        }
    }
}

fn alloc_adt(tag: i64, fields: &[i64]) -> *mut u8 {
    let nbytes = (1 + fields.len() as u64) * 8;
    let dest = lumia_alloc(nbytes, TYPE_ADT);
    if dest.is_null() {
        panic!("lumia: adt OOM");
    }
    unsafe {
        let dst = dest as *mut i64;
        *dst = tag;
        for (i, f) in fields.iter().enumerate() {
            *dst.add(1 + i) = *f;
        }
    }
    dest
}

/// Immutable upsert: new Map with `key → val` (overwrite keeps insertion slot).
#[no_mangle]
pub extern "C" fn lumia_map_set(map: *mut u8, key: i64, val: i64) -> *mut u8 {
    unsafe {
        let (n, base) = if map.is_null() {
            (0i64, ptr::null())
        } else {
            (*(map as *const i64), map as *const i64)
        };
        if let Some(i) = map_find(map, key) {
            let nbytes = (1 + n as u64 * 2) * 8;
            let dest = lumia_alloc(nbytes, TYPE_MAP);
            if dest.is_null() {
                panic!("lumia: map set OOM");
            }
            let dst = dest as *mut i64;
            *dst = n;
            for j in 0..(n as usize * 2) {
                *dst.add(1 + j) = *base.add(1 + j);
            }
            *dst.add(2 + i * 2) = val;
            dest
        } else {
            let n2 = n + 1;
            let nbytes = (1 + n2 as u64 * 2) * 8;
            let dest = lumia_alloc(nbytes, TYPE_MAP);
            if dest.is_null() {
                panic!("lumia: map set OOM");
            }
            let dst = dest as *mut i64;
            *dst = n2;
            for j in 0..(n as usize * 2) {
                *dst.add(1 + j) = *base.add(1 + j);
            }
            *dst.add(1 + n as usize * 2) = key;
            *dst.add(2 + n as usize * 2) = val;
            dest
        }
    }
}

/// Drop key if present; returns new Map (insertion order of remaining keys).
#[no_mangle]
pub extern "C" fn lumia_map_remove(map: *mut u8, key: i64) -> *mut u8 {
    unsafe {
        let (n, base) = if map.is_null() {
            (0i64, ptr::null())
        } else {
            (*(map as *const i64), map as *const i64)
        };
        let Some(idx) = map_find(map, key) else {
            // unchanged — copy
            if map.is_null() {
                let dest = lumia_alloc(8, TYPE_MAP);
                *(dest as *mut i64) = 0;
                return dest;
            }
            let nbytes = (1 + n as u64 * 2) * 8;
            let dest = lumia_alloc(nbytes, TYPE_MAP);
            ptr::copy_nonoverlapping(map, dest, nbytes as usize);
            return dest;
        };
        let n2 = n - 1;
        let nbytes = (1 + n2 as u64 * 2) * 8;
        let dest = lumia_alloc(nbytes, TYPE_MAP);
        if dest.is_null() {
            panic!("lumia: map remove OOM");
        }
        let dst = dest as *mut i64;
        *dst = n2;
        let mut w = 0usize;
        for j in 0..n as usize {
            if j == idx {
                continue;
            }
            *dst.add(1 + w * 2) = *base.add(1 + j * 2);
            *dst.add(2 + w * 2) = *base.add(2 + j * 2);
            w += 1;
        }
        dest
    }
}

/// Keys in insertion order as HeapList.
#[no_mangle]
pub extern "C" fn lumia_map_keys(map: *mut u8) -> *mut u8 {
    unsafe {
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = (1 + n as u64) * 8;
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        if dest.is_null() {
            panic!("lumia: map keys OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            let base = map as *const i64;
            for i in 0..n as usize {
                *dst.add(1 + i) = *base.add(1 + i * 2);
            }
        }
        dest
    }
}

/// Values in insertion order as HeapList.
#[no_mangle]
pub extern "C" fn lumia_map_values(map: *mut u8) -> *mut u8 {
    unsafe {
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = (1 + n as u64) * 8;
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        if dest.is_null() {
            panic!("lumia: map values OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            let base = map as *const i64;
            for i in 0..n as usize {
                *dst.add(1 + i) = *base.add(2 + i * 2);
            }
        }
        dest
    }
}

/// Insertion-ordered list of `(k, v)` pairs (each pair is ADT tag0 + 2 fields).
#[no_mangle]
pub extern "C" fn lumia_map_items(map: *mut u8) -> *mut u8 {
    unsafe {
        let n = if map.is_null() {
            0i64
        } else {
            *(map as *const i64)
        };
        let nbytes = (1 + n as u64) * 8;
        let dest = lumia_alloc(nbytes, TYPE_LIST);
        if dest.is_null() {
            panic!("lumia: map items OOM");
        }
        let dst = dest as *mut i64;
        *dst = n;
        if !map.is_null() {
            let base = map as *const i64;
            for i in 0..n as usize {
                let k = *base.add(1 + i * 2);
                let v = *base.add(2 + i * 2);
                let pair = alloc_adt(0, &[k, v]);
                *dst.add(1 + i) = pair as i64;
            }
        }
        dest
    }
}

/// Set payload shares list layout `[len][e0]...`.
#[no_mangle]
pub extern "C" fn lumia_set_contains(set: *mut u8, elem: i64) -> i64 {
    if set.is_null() {
        return 0;
    }
    unsafe {
        let n = *(set as *const i64);
        let base = set as *const i64;
        for i in 0..n as usize {
            if *base.add(1 + i) == elem {
                return 1;
            }
        }
        0
    }
}

/// Dispatch get: List/Set by index → i64 elem; Map by key → Option ADT ptr as i64.
#[no_mangle]
pub extern "C" fn lumia_get(
    obj: *mut u8,
    key_or_index: i64,
    some_tag: i64,
    none_tag: i64,
) -> i64 {
    if obj.is_null() {
        panic!("lumia: get on null");
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            TYPE_LIST | TYPE_SET => lumia_list_get(obj, key_or_index),
            TYPE_MAP => {
                let opt = lumia_map_get(obj, key_or_index, some_tag, none_tag);
                opt as i64
            }
            other => panic!("lumia: get unsupported type_id {other}"),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_contains(obj: *mut u8, key: i64) -> i64 {
    if obj.is_null() {
        return 0;
    }
    let h = header_from_payload(obj);
    unsafe {
        match (*h).type_id {
            TYPE_MAP => lumia_map_contains(obj, key),
            TYPE_SET => lumia_set_contains(obj, key),
            other => panic!("lumia: contains unsupported type_id {other}"),
        }
    }
}

#[no_mangle]
pub extern "C" fn lumia_adt_tag(obj: *mut u8) -> i64 {
    if obj.is_null() {
        panic!("lumia: adt_tag on null");
    }
    unsafe { *(obj as *const i64) }
}

#[no_mangle]
pub extern "C" fn lumia_adt_field(obj: *mut u8, index: i64) -> i64 {
    if obj.is_null() || index < 0 {
        panic!("lumia: adt_field OOB");
    }
    unsafe {
        let base = obj as *const i64;
        *base.add(1 + index as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_collect_unrooted() {
        let p = lumia_alloc(16, TYPE_BYTES);
        assert!(!p.is_null());
        // Not rooted → collect should free
        lumia_gc_collect();
        // Heap should be empty or not contain live unmarked — allocate again
        let q = lumia_alloc(8, TYPE_BYTES);
        assert!(!q.is_null());
    }

    #[test]
    fn rooted_survives_collect() {
        let mut slot: *mut u8 = lumia_alloc(32, TYPE_STRING);
        lumia_root_push(&mut slot as *mut *mut u8);
        lumia_gc_collect();
        assert!(!slot.is_null());
        // header still valid
        let h = header_from_payload(slot);
        unsafe {
            assert_eq!((*h).type_id, TYPE_STRING);
            assert_eq!((*h).size, 32);
        }
        lumia_root_pop();
    }

    #[test]
    fn write_barrier_noop() {
        let p = lumia_alloc(8, TYPE_BYTES);
        lumia_write_barrier(p, 0, ptr::null_mut());
    }

    #[test]
    fn println_int_smoke() {
        lumia_println_int(7);
    }
}


//! Process bump nursery for small young allocations.
//!
//! Minor GC **evacuates** nursery survivors into old (system `alloc`); the slab
//! then rewinds. Dead nursery slots use a free sentinel for lock-free probes.
//!
//! Mutators may **claim a TLS LAB** ([`Nursery::claim_lab`]) carved from this
//! slab and bump without the heap lock; pending objects flush into `h.young`
//! before STW. Membership: nursery objects live in [`Nursery::live_set`] and/or
//! as initialized headers below the bump cursor (unflushed LAB). Not `heap_set`.
//! Only the process heap may [`Nursery::publish_range`].

use std::alloc::{alloc, dealloc, Layout};
use std::sync::atomic::{AtomicUsize, Ordering};

use rustc_hash::FxHashSet;

use crate::common::{header_layout, ObjectHeader};
use crate::heap::DEFAULT_YOUNG_LIMIT;

/// Process nursery `[base, end)` / bump high-water — set by [`Nursery::publish_range`]
/// and updated on bump / rewind.
static NURSERY_BASE: AtomicUsize = AtomicUsize::new(0);
static NURSERY_END: AtomicUsize = AtomicUsize::new(0);
static NURSERY_CURSOR: AtomicUsize = AtomicUsize::new(0);

/// Dead freelist / abandoned nursery slot (not a real Lumia type_id).
pub(crate) const TYPE_NURSERY_FREE: u32 = 0xFFFF_FFFE;
/// Temporary forwarding during evacuating minor (STW only; slab rewound after).
pub(crate) const TYPE_NURSERY_FWD: u32 = 0xFFFF_FFFD;

/// Default TLS LAB claim size (bytes of slab, including headers).
pub(crate) const LAB_CLAIM_BYTES: usize = 4096;

/// True when `h` lies in the published process nursery slab.
#[inline]
pub(crate) fn nursery_range_contains_header(h: *mut ObjectHeader) -> bool {
    if h.is_null() {
        return false;
    }
    let p = h as usize;
    let b = NURSERY_BASE.load(Ordering::Relaxed);
    let e = NURSERY_END.load(Ordering::Relaxed);
    b != 0 && p >= b && p < e
}

/// Lock-free nursery membership: in published range, below cursor, not FREE/FWD.
///
/// May false-negative under concurrent bump (cursor races); callers that need
/// exact answers under mutation still use the heap lock. Safe false-negative.
#[inline]
pub(crate) fn nursery_probe_live_header(h: *mut ObjectHeader) -> Option<bool> {
    if !nursery_range_contains_header(h) {
        return None;
    }
    let p = h as usize;
    let b = NURSERY_BASE.load(Ordering::Relaxed);
    let cur = NURSERY_CURSOR.load(Ordering::Relaxed);
    if p >= b.wrapping_add(cur) {
        return Some(false);
    }
    // Interior / immediate bits may land in-range; headers are 8-byte aligned.
    if !p.is_multiple_of(8) {
        return Some(false);
    }
    // SAFETY: `h` is an aligned address in the process nursery slab we own.
    let tid = unsafe { core::ptr::addr_of!((*h).type_id).read_volatile() };
    if tid == TYPE_NURSERY_FREE {
        return Some(false);
    }
    // `0` = freelist slot taken but header not yet initialized; FWD = mid-evacuate.
    // Both require the heap lock for an exact answer (safe false-negative via None).
    if tid == 0 || tid == TYPE_NURSERY_FWD {
        return None;
    }
    Some(true)
}

pub(crate) struct Nursery {
    base: *mut u8,
    capacity: usize,
    cursor: usize,
    live: usize,
    freelist: Vec<*mut ObjectHeader>,
    live_set: FxHashSet<*mut ObjectHeader>,
    published: bool,
}

unsafe impl Send for Nursery {}
unsafe impl Sync for Nursery {}

impl Nursery {
    pub(crate) fn new() -> Self {
        Self::with_capacity(DEFAULT_YOUNG_LIMIT)
    }

    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(64).next_multiple_of(8);
        let layout = Layout::from_size_align(capacity, 8).unwrap_or_else(|_| {
            Layout::from_size_align(DEFAULT_YOUNG_LIMIT, 8).expect("nursery layout")
        });
        let base = unsafe { alloc(layout) };
        if base.is_null() {
            return Self {
                base: std::ptr::null_mut(),
                capacity: 0,
                cursor: 0,
                live: 0,
                freelist: Vec::new(),
                live_set: FxHashSet::default(),
                published: false,
            };
        }
        Self {
            base,
            capacity,
            cursor: 0,
            live: 0,
            freelist: Vec::new(),
            live_set: FxHashSet::default(),
            published: false,
        }
    }

    pub(crate) fn publish_range(&mut self) {
        if self.base.is_null() || self.capacity == 0 {
            NURSERY_BASE.store(0, Ordering::Relaxed);
            NURSERY_END.store(0, Ordering::Relaxed);
            NURSERY_CURSOR.store(0, Ordering::Relaxed);
            self.published = false;
            return;
        }
        let end = (self.base as usize).wrapping_add(self.capacity);
        NURSERY_BASE.store(self.base as usize, Ordering::Relaxed);
        NURSERY_END.store(end, Ordering::Relaxed);
        NURSERY_CURSOR.store(self.cursor, Ordering::Relaxed);
        self.published = true;
    }

    fn sync_cursor_atomic(&self) {
        if self.published {
            NURSERY_CURSOR.store(self.cursor, Ordering::Relaxed);
        }
    }

    #[inline]
    pub(crate) fn contains_header(&self, h: *mut ObjectHeader) -> bool {
        if self.base.is_null() || h.is_null() {
            return false;
        }
        let p = h as usize;
        let b = self.base as usize;
        p >= b && p < b.wrapping_add(self.capacity)
    }

    /// Live nursery object: in `live_set`, or an initialized header below cursor
    /// (unflushed TLS LAB bump — not yet published into `live_set` / `h.young`).
    #[inline]
    pub(crate) fn is_live(&self, h: *mut ObjectHeader) -> bool {
        if self.live_set.contains(&h) {
            return true;
        }
        if !self.contains_header(h) {
            return false;
        }
        // Field words / immediates may land in the slab range; only headers are aligned.
        if !(h as usize).is_multiple_of(8) {
            return false;
        }
        let off = (h as usize).wrapping_sub(self.base as usize);
        if off >= self.cursor {
            return false;
        }
        // SAFETY: `h` is an aligned address inside this nursery slab.
        let tid = unsafe { (*h).type_id };
        tid != 0 && tid != TYPE_NURSERY_FREE && tid != TYPE_NURSERY_FWD
    }

    /// Drop a live nursery object from membership (evacuate / pre-rewind).
    pub(crate) fn forget_live(&mut self, h: *mut ObjectHeader) {
        if self.live_set.remove(&h) {
            self.live = self.live.saturating_sub(1);
        }
    }

    /// Record a flushed TLS LAB object into `live_set` (caller also `insert_young`).
    pub(crate) fn note_flushed(&mut self, h: *mut ObjectHeader) {
        if self.live_set.insert(h) {
            self.live = self.live.saturating_add(1);
        }
    }

    #[inline]
    fn total_bytes(payload: usize) -> Option<usize> {
        let layout = header_layout(payload);
        Some(layout.size().next_multiple_of(8))
    }

    /// Carve a contiguous LAB for TLS bump (caller holds heap). Advances cursor
    /// so lock-free probes cover the whole claim; does not touch `live_set`.
    pub(crate) fn claim_lab(&mut self, bytes: usize) -> Option<(*mut u8, usize)> {
        if self.base.is_null() {
            return None;
        }
        let bytes = bytes.max(64).next_multiple_of(8);
        let next = self.cursor.checked_add(bytes)?;
        if next > self.capacity {
            return None;
        }
        let start = unsafe { self.base.add(self.cursor) };
        self.cursor = next;
        self.sync_cursor_atomic();
        Some((start, bytes))
    }

    pub(crate) unsafe fn try_alloc(&mut self, payload: usize) -> Option<*mut ObjectHeader> {
        if self.base.is_null() {
            return None;
        }
        let total = Self::total_bytes(payload)?;

        if let Some(i) = self
            .freelist
            .iter()
            .position(|&h| unsafe { (*h).size as usize == payload })
        {
            let h = self.freelist.swap_remove(i);
            // Clear FREE so lock-free probes do not treat a reclaimed slot as dead
            // before `init_alloc_header` runs (tid==0 → probe returns None).
            (*h).type_id = 0;
            self.live = self.live.saturating_add(1);
            self.live_set.insert(h);
            return Some(h);
        }

        let next = self.cursor.checked_add(total)?;
        if next > self.capacity {
            return None;
        }
        let mem = self.base.add(self.cursor);
        self.cursor = next;
        self.sync_cursor_atomic();
        self.live = self.live.saturating_add(1);
        let h = mem as *mut ObjectHeader;
        self.live_set.insert(h);
        Some(h)
    }

    pub(crate) unsafe fn free(&mut self, h: *mut ObjectHeader) {
        debug_assert!(self.contains_header(h));
        self.live_set.remove(&h);
        (*h).type_id = TYPE_NURSERY_FREE;
        self.live = self.live.saturating_sub(1);
        if self.live == 0 {
            self.rewind();
            return;
        }
        self.freelist.push(h);
    }

    /// Abandon the slab contents (after evacuating minor or when empty).
    pub(crate) fn rewind(&mut self) {
        self.cursor = 0;
        self.live = 0;
        self.freelist.clear();
        self.live_set.clear();
        self.sync_cursor_atomic();
    }
}

impl Drop for Nursery {
    fn drop(&mut self) {
        if self.published {
            if NURSERY_BASE.load(Ordering::Relaxed) == self.base as usize {
                NURSERY_BASE.store(0, Ordering::Relaxed);
                NURSERY_END.store(0, Ordering::Relaxed);
                NURSERY_CURSOR.store(0, Ordering::Relaxed);
            }
            self.published = false;
        }
        if self.base.is_null() || self.capacity == 0 {
            return;
        }
        let layout = Layout::from_size_align(self.capacity, 8).unwrap_or_else(|_| {
            Layout::from_size_align(DEFAULT_YOUNG_LIMIT, 8).expect("nursery drop layout")
        });
        unsafe { dealloc(self.base, layout) };
        self.base = std::ptr::null_mut();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_then_rewind_when_empty() {
        let mut n = Nursery::with_capacity(256);
        let a = unsafe { n.try_alloc(16) }.expect("a");
        let b = unsafe { n.try_alloc(16) }.expect("b");
        unsafe {
            (*a).size = 16;
            (*b).size = 16;
            n.free(a);
            n.free(b);
        }
        assert_eq!(n.live, 0);
        assert_eq!(n.cursor, 0);
        let c = unsafe { n.try_alloc(16) }.expect("reuse after rewind");
        assert_eq!(c, n.base as *mut ObjectHeader);
    }

    #[test]
    fn freelist_reuses_exact_size() {
        let mut n = Nursery::with_capacity(512);
        let a = unsafe { n.try_alloc(24) }.expect("a");
        let _b = unsafe { n.try_alloc(8) }.expect("b");
        unsafe {
            (*a).size = 24;
            n.free(a);
        }
        assert_eq!(unsafe { (*a).type_id }, TYPE_NURSERY_FREE);
        let c = unsafe { n.try_alloc(24) }.expect("freelist");
        assert_eq!(c, a);
    }

    #[test]
    fn claim_lab_advances_cursor() {
        let mut n = Nursery::with_capacity(8192);
        let (p, len) = n.claim_lab(4096).expect("lab");
        assert_eq!(len, 4096);
        assert_eq!(p, n.base);
        assert_eq!(n.cursor, 4096);
        assert!(n.live_set.is_empty());
        let (p2, _) = n.claim_lab(1024).expect("lab2");
        assert_eq!(p2, unsafe { n.base.add(4096) });
    }

    #[test]
    fn unpublished_drop_does_not_clear_process_atomics() {
        let mut process = Nursery::with_capacity(128);
        process.publish_range();
        let b0 = NURSERY_BASE.load(Ordering::Relaxed);
        assert_ne!(b0, 0);
        {
            let _temp = Nursery::with_capacity(64);
        }
        assert_eq!(NURSERY_BASE.load(Ordering::Relaxed), b0);
        drop(process);
        assert_eq!(NURSERY_BASE.load(Ordering::Relaxed), 0);
    }
}

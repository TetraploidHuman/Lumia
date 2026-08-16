//! Transparent Memo `T_f` tables (DESIGN §7.5).

use crate::heap::with_heap;
use crate::common::trap_abort;
use crate::{MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS, MEMO_TF_MAX_ARGS, MEMO_TF_MAX_FUNS, MEMO_TF_SLOTS};
use std::cell::{Cell, RefCell};

/// Transparent Memo `T_f` — fixed small associative tables (DESIGN §7.5.1-B).
/// Caps live in `lumia_abi` (`MEMO_TF_*`). C entry points stay
/// `lumia_memo_l2_*` for ABI stability (DESIGN vocabulary is `T_f`, not L2).

#[derive(Clone, Copy)]
struct MemoTfSlot {
    valid: bool,
    nargs: u8,
    args: [i64; MEMO_TF_MAX_ARGS],
    result: i64,
}

impl MemoTfSlot {
    const EMPTY: Self = Self {
        valid: false,
        nargs: 0,
        args: [0; MEMO_TF_MAX_ARGS],
        result: 0,
    };

    fn matches(&self, nargs: u8, args: &[i64; MEMO_TF_MAX_ARGS]) -> bool {
        self.valid && self.nargs == nargs && self.args[..nargs as usize] == args[..nargs as usize]
    }
}

struct MemoTfTable {
    slots: [MemoTfSlot; MEMO_TF_SLOTS],
    next_victim: Cell<usize>,
    hits: Cell<u64>,
    misses: Cell<u64>,
}

impl MemoTfTable {
    fn empty() -> Self {
        Self {
            slots: [MemoTfSlot::EMPTY; MEMO_TF_SLOTS],
            next_victim: Cell::new(0),
            hits: Cell::new(0),
            misses: Cell::new(0),
        }
    }
}

thread_local! {
    static MEMO_TF: RefCell<[MemoTfTable; MEMO_TF_MAX_FUNS]> =
        RefCell::new(std::array::from_fn(|_| MemoTfTable::empty()));
}

fn pack_args(a0: i64, a1: i64, a2: i64, a3: i64) -> [i64; MEMO_TF_MAX_ARGS] {
    [a0, a1, a2, a3]
}

/// Lookup: returns 1 and writes `*out_result` on hit; else 0.
///
/// Uses shared `borrow` (counters are `Cell`) so hot miss/hit paths avoid exclusive locks.
/// Rust path (not `extern "C"`) so `trap_abort` can unwind under `cfg(test)`.
pub(crate) fn memo_l2_lookup(
    fun_id: i64,
    nargs: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    out_result: *mut i64,
) -> i64 {
    ensure_memo_registered();
    if out_result.is_null() {
        trap_abort("lumia: memo lookup with null out_result");
    }
    if fun_id < 0 || fun_id as usize >= MEMO_TF_MAX_FUNS {
        trap_abort(&format!(
            "lumia: memo lookup bad fun_id={fun_id} (max {MEMO_TF_MAX_FUNS})"
        ));
    }
    let nargs = nargs.clamp(0, MEMO_TF_MAX_ARGS as i64) as u8;
    let args = pack_args(a0, a1, a2, a3);
    // Serialize with GC mark walks (heap lock).
    with_heap(|_| {
        MEMO_TF.with(|t| {
            let tables = t.borrow();
            let table = &tables[fun_id as usize];
            for slot in &table.slots {
                if slot.matches(nargs, &args) {
                    table.hits.set(table.hits.get() + 1);
                    unsafe {
                        *out_result = slot.result;
                    }
                    return 1;
                }
            }
            table.misses.set(table.misses.get() + 1);
            0
        })
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_l2_lookup(
    fun_id: i64,
    nargs: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    out_result: *mut i64,
) -> i64 {
    memo_l2_lookup(fun_id, nargs, a0, a1, a2, a3, out_result)
}

/// Store result into a slot (round-robin eviction).
#[no_mangle]
pub extern "C" fn lumia_memo_l2_store(
    fun_id: i64,
    nargs: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    result: i64,
) {
    ensure_memo_registered();
    if fun_id < 0 || fun_id as usize >= MEMO_TF_MAX_FUNS {
        trap_abort(&format!(
            "lumia: memo store bad fun_id={fun_id} (max {MEMO_TF_MAX_FUNS})"
        ));
    }
    let nargs = nargs.clamp(0, MEMO_TF_MAX_ARGS as i64) as u8;
    let args = pack_args(a0, a1, a2, a3);
    with_heap(|h| {
        let full = h.full_marking;
        MEMO_TF.with(|t| {
            let mut tables = t.borrow_mut();
            let table = &mut tables[fun_id as usize];
            for slot in &mut table.slots {
                if slot.matches(nargs, &args) {
                    slot.result = result;
                    if full {
                        crate::gc::mark_value(result);
                    }
                    return;
                }
            }
            let i = table.next_victim.get() % MEMO_TF_SLOTS;
            table.next_victim.set(i + 1);
            let mut stored = [0i64; MEMO_TF_MAX_ARGS];
            stored[..nargs as usize].copy_from_slice(&args[..nargs as usize]);
            table.slots[i] = MemoTfSlot {
                valid: true,
                nargs,
                args: stored,
                result,
            };
            if full {
                for a in stored.iter().take(nargs as usize) {
                    crate::gc::mark_value(*a);
                }
                crate::gc::mark_value(result);
            }
        });
    });
}

/// Test / `--show-memo-stats` helper: total hits across tables.
#[no_mangle]
pub extern "C" fn lumia_memo_l2_hits() -> i64 {
    with_heap(|_| MEMO_TF.with(|t| t.borrow().iter().map(|x| x.hits.get() as i64).sum()))
}

#[no_mangle]
pub extern "C" fn lumia_memo_l2_misses() -> i64 {
    with_heap(|_| MEMO_TF.with(|t| t.borrow().iter().map(|x| x.misses.get() as i64).sum()))
}

#[no_mangle]
pub extern "C" fn lumia_memo_l2_reset() {
    with_heap(|_| {
        MEMO_TF.with(|t| {
            *t.borrow_mut() = std::array::from_fn(|_| MemoTfTable::empty());
        });
    });
}

/// Dense Int-key `T_f` for structural recursion (DESIGN §7.5.3) — prefer over hashing.
struct MemoIdxTable {
    valid: [u8; MEMO_IDX_CAP],
    values: [i64; MEMO_IDX_CAP],
    hits: Cell<u64>,
    misses: Cell<u64>,
}

impl MemoIdxTable {
    fn new() -> Box<Self> {
        Box::new(Self {
            valid: [0; MEMO_IDX_CAP],
            values: [0; MEMO_IDX_CAP],
            hits: Cell::new(0),
            misses: Cell::new(0),
        })
    }
}

thread_local! {
    // Lazy: allocate a dense table only on first *store* of that `fun_id` (§7.5 low occupancy).
    static MEMO_IDX: RefCell<[Option<Box<MemoIdxTable>>; MEMO_IDX_MAX_FUNS]> =
        const { RefCell::new([const { None }; MEMO_IDX_MAX_FUNS]) };
    static MEMO_REGISTRATION: MemoRegistration = MemoRegistration::new();
}

struct MemoEntry {
    tf: *const RefCell<[MemoTfTable; MEMO_TF_MAX_FUNS]>,
    idx: *const RefCell<[Option<Box<MemoIdxTable>>; MEMO_IDX_MAX_FUNS]>,
}

unsafe impl Send for MemoEntry {}
unsafe impl Sync for MemoEntry {}

static MEMO_REGISTRY: std::sync::OnceLock<std::sync::Mutex<Vec<MemoEntry>>> =
    std::sync::OnceLock::new();

fn memo_registry() -> &'static std::sync::Mutex<Vec<MemoEntry>> {
    MEMO_REGISTRY.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

struct MemoRegistration;

impl MemoRegistration {
    fn new() -> Self {
        let tf = MEMO_TF.with(|t| t as *const _);
        let idx = MEMO_IDX.with(|t| t as *const _);
        with_heap(|_| {
            memo_registry()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(MemoEntry { tf, idx });
        });
        Self
    }
}

impl Drop for MemoRegistration {
    fn drop(&mut self) {
        with_heap(|_| {
            let Ok(tf) = MEMO_TF.try_with(|t| t as *const _) else {
                return;
            };
            if let Ok(mut reg) = memo_registry().lock() {
                reg.retain(|e| e.tf != tf);
            }
        });
    }
}

fn ensure_memo_registered() {
    MEMO_REGISTRATION.with(|_| {});
}

fn walk_memo_tables(
    tf: &RefCell<[MemoTfTable; MEMO_TF_MAX_FUNS]>,
    idx: &RefCell<[Option<Box<MemoIdxTable>>; MEMO_IDX_MAX_FUNS]>,
    mut f: impl FnMut(i64),
) {
    // Caller holds the heap lock ⇒ mutators cannot be in memo store/lookup.
    let tables = tf.borrow();
    for table in tables.iter() {
        for slot in &table.slots {
            if !slot.valid {
                continue;
            }
            for a in slot.args.iter().take(slot.nargs as usize) {
                f(*a);
            }
            f(slot.result);
        }
    }
    let tables = idx.borrow();
    for table in tables.iter().flatten() {
        for (i, &v) in table.valid.iter().enumerate() {
            if v != 0 {
                f(table.values[i]);
            }
        }
    }
}

/// Walk memo table slots so GC can mark heap bits retained by `T_f` (all mutators).
pub(crate) fn for_each_memo_i64(mut f: impl FnMut(i64)) {
    ensure_memo_registered();
    let entries: Vec<MemoEntry> = memo_registry()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|e| MemoEntry {
            tf: e.tf,
            idx: e.idx,
        })
        .collect();
    for e in entries {
        unsafe {
            walk_memo_tables(&*e.tf, &*e.idx, &mut f);
        }
    }
}

fn memo_idx_table(
    tables: &mut [Option<Box<MemoIdxTable>>; MEMO_IDX_MAX_FUNS],
    fun_id: usize,
) -> &mut MemoIdxTable {
    if tables[fun_id].is_none() {
        tables[fun_id] = Some(MemoIdxTable::new());
    }
    tables[fun_id].as_mut().unwrap()
}

/// Lookup by Int key in `0..MEMO_IDX_CAP`. Returns 1 + writes result on hit.
///
/// Does not allocate on miss (table created on first store).
/// Rust path (not `extern "C"`) so `trap_abort` can unwind under `cfg(test)`.
pub(crate) fn memo_idx_lookup(fun_id: i64, key: i64, out_result: *mut i64) -> i64 {
    ensure_memo_registered();
    if out_result.is_null() {
        trap_abort("lumia: memo idx lookup with null out_result");
    }
    if fun_id < 0 || fun_id as usize >= MEMO_IDX_MAX_FUNS {
        trap_abort(&format!(
            "lumia: memo idx lookup bad fun_id={fun_id} (max {MEMO_IDX_MAX_FUNS})"
        ));
    }
    // Key outside the dense domain is a cold miss (not a planning bug).
    if key < 0 || key as usize >= MEMO_IDX_CAP {
        return 0;
    }
    let k = key as usize;
    with_heap(|_| {
        MEMO_IDX.with(|t| {
            let tables = t.borrow();
            let Some(table) = tables[fun_id as usize].as_ref() else {
                return 0;
            };
            if table.valid[k] != 0 {
                table.hits.set(table.hits.get() + 1);
                unsafe {
                    *out_result = table.values[k];
                }
                1
            } else {
                table.misses.set(table.misses.get() + 1);
                0
            }
        })
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_lookup(fun_id: i64, key: i64, out_result: *mut i64) -> i64 {
    memo_idx_lookup(fun_id, key, out_result)
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_store(fun_id: i64, key: i64, result: i64) {
    ensure_memo_registered();
    if fun_id < 0 || fun_id as usize >= MEMO_IDX_MAX_FUNS {
        trap_abort(&format!(
            "lumia: memo idx store bad fun_id={fun_id} (max {MEMO_IDX_MAX_FUNS})"
        ));
    }
    if key < 0 || key as usize >= MEMO_IDX_CAP {
        trap_abort(&format!(
            "lumia: memo idx store key={key} out of dense domain (cap {MEMO_IDX_CAP})"
        ));
    }
    let k = key as usize;
    with_heap(|h| {
        let full = h.full_marking;
        MEMO_IDX.with(|t| {
            let mut tables = t.borrow_mut();
            let table = memo_idx_table(&mut tables, fun_id as usize);
            table.valid[k] = 1;
            table.values[k] = result;
        });
        if full {
            crate::gc::mark_value(result);
        }
    });
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_hits() -> i64 {
    with_heap(|_| {
        MEMO_IDX.with(|t| {
            t.borrow()
                .iter()
                .filter_map(|x| x.as_ref())
                .map(|x| x.hits.get() as i64)
                .sum()
        })
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_misses() -> i64 {
    with_heap(|_| {
        MEMO_IDX.with(|t| {
            t.borrow()
                .iter()
                .filter_map(|x| x.as_ref())
                .map(|x| x.misses.get() as i64)
                .sum()
        })
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_reset() {
    with_heap(|_| {
        MEMO_IDX.with(|t| {
            *t.borrow_mut() = [const { None }; MEMO_IDX_MAX_FUNS];
        });
    });
}

//! Transparent Memo `T_f` tables (DESIGN §7.5).

use crate::{MEMO_IDX_CAP, MEMO_IDX_MAX_FUNS, MEMO_L2_MAX_ARGS, MEMO_L2_MAX_FUNS, MEMO_L2_SLOTS};
use std::cell::RefCell;

/// Transparent Memo `T_f` — fixed small associative tables (DESIGN §7.5.1-B).
/// Caps live in `lumia_abi` (`MEMO_TF_*` / `MEMO_L2_*`). C entry points stay
/// `lumia_memo_l2_*` for ABI stability (DESIGN vocabulary is `T_f`, not L2).

#[derive(Clone, Copy)]
struct MemoL2Slot {
    valid: bool,
    nargs: u8,
    args: [i64; MEMO_L2_MAX_ARGS],
    result: i64,
}

impl MemoL2Slot {
    const EMPTY: Self = Self {
        valid: false,
        nargs: 0,
        args: [0; MEMO_L2_MAX_ARGS],
        result: 0,
    };

    fn matches(&self, nargs: u8, args: &[i64; MEMO_L2_MAX_ARGS]) -> bool {
        self.valid && self.nargs == nargs && self.args[..nargs as usize] == args[..nargs as usize]
    }
}

struct MemoL2Table {
    slots: [MemoL2Slot; MEMO_L2_SLOTS],
    next_victim: usize,
    hits: u64,
    misses: u64,
}

impl MemoL2Table {
    const EMPTY: Self = Self {
        slots: [MemoL2Slot::EMPTY; MEMO_L2_SLOTS],
        next_victim: 0,
        hits: 0,
        misses: 0,
    };
}

thread_local! {
    static MEMO_L2: RefCell<[MemoL2Table; MEMO_L2_MAX_FUNS]> =
        const { RefCell::new([MemoL2Table::EMPTY; MEMO_L2_MAX_FUNS]) };
}

fn pack_args(a0: i64, a1: i64, a2: i64, a3: i64) -> [i64; MEMO_L2_MAX_ARGS] {
    [a0, a1, a2, a3]
}

/// Lookup: returns 1 and writes `*out_result` on hit; else 0.
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
    if fun_id < 0 || fun_id as usize >= MEMO_L2_MAX_FUNS || out_result.is_null() {
        return 0;
    }
    let nargs = nargs.clamp(0, MEMO_L2_MAX_ARGS as i64) as u8;
    let args = pack_args(a0, a1, a2, a3);
    MEMO_L2.with(|t| {
        let mut tables = t.borrow_mut();
        let table = &mut tables[fun_id as usize];
        for slot in &table.slots {
            if slot.matches(nargs, &args) {
                table.hits += 1;
                unsafe {
                    *out_result = slot.result;
                }
                return 1;
            }
        }
        table.misses += 1;
        0
    })
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
    if fun_id < 0 || fun_id as usize >= MEMO_L2_MAX_FUNS {
        return;
    }
    let nargs = nargs.clamp(0, MEMO_L2_MAX_ARGS as i64) as u8;
    let args = pack_args(a0, a1, a2, a3);
    MEMO_L2.with(|t| {
        let mut tables = t.borrow_mut();
        let table = &mut tables[fun_id as usize];
        for slot in &mut table.slots {
            if slot.matches(nargs, &args) {
                slot.result = result;
                return;
            }
        }
        let i = table.next_victim % MEMO_L2_SLOTS;
        table.next_victim = i + 1;
        let mut stored = [0i64; MEMO_L2_MAX_ARGS];
        stored[..nargs as usize].copy_from_slice(&args[..nargs as usize]);
        table.slots[i] = MemoL2Slot {
            valid: true,
            nargs,
            args: stored,
            result,
        };
    });
}

/// Test / `--show-memo-stats` helper: total hits across tables.
#[no_mangle]
pub extern "C" fn lumia_memo_l2_hits() -> i64 {
    MEMO_L2.with(|t| t.borrow().iter().map(|x| x.hits as i64).sum())
}

#[no_mangle]
pub extern "C" fn lumia_memo_l2_misses() -> i64 {
    MEMO_L2.with(|t| t.borrow().iter().map(|x| x.misses as i64).sum())
}

#[no_mangle]
pub extern "C" fn lumia_memo_l2_reset() {
    MEMO_L2.with(|t| {
        *t.borrow_mut() = [MemoL2Table::EMPTY; MEMO_L2_MAX_FUNS];
    });
}

/// Dense Int-key `T_f` for structural recursion (DESIGN §7.5.3) — prefer over hashing.
struct MemoIdxTable {
    valid: [u8; MEMO_IDX_CAP],
    values: [i64; MEMO_IDX_CAP],
    hits: u64,
    misses: u64,
}

impl MemoIdxTable {
    fn new() -> Box<Self> {
        Box::new(Self {
            valid: [0; MEMO_IDX_CAP],
            values: [0; MEMO_IDX_CAP],
            hits: 0,
            misses: 0,
        })
    }
}

thread_local! {
    // Lazy: allocate a dense table only on first use of that `fun_id` (§7.5 low occupancy).
    static MEMO_IDX: RefCell<[Option<Box<MemoIdxTable>>; MEMO_IDX_MAX_FUNS]> =
        const { RefCell::new([const { None }; MEMO_IDX_MAX_FUNS]) };
}

/// Walk memo table slots so GC can mark heap bits retained by `T_f`.
pub(crate) fn for_each_memo_i64(mut f: impl FnMut(i64)) {
    MEMO_L2.with(|t| {
        for table in t.borrow().iter() {
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
    });
    MEMO_IDX.with(|t| {
        for table in t.borrow().iter().flatten() {
            for (i, &v) in table.valid.iter().enumerate() {
                if v != 0 {
                    f(table.values[i]);
                }
            }
        }
    });
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
#[no_mangle]
pub extern "C" fn lumia_memo_idx_lookup(fun_id: i64, key: i64, out_result: *mut i64) -> i64 {
    if fun_id < 0
        || fun_id as usize >= MEMO_IDX_MAX_FUNS
        || out_result.is_null()
        || key < 0
        || key as usize >= MEMO_IDX_CAP
    {
        return 0;
    }
    let k = key as usize;
    MEMO_IDX.with(|t| {
        let mut tables = t.borrow_mut();
        let table = memo_idx_table(&mut tables, fun_id as usize);
        if table.valid[k] != 0 {
            table.hits += 1;
            unsafe {
                *out_result = table.values[k];
            }
            1
        } else {
            table.misses += 1;
            0
        }
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_store(fun_id: i64, key: i64, result: i64) {
    if fun_id < 0 || fun_id as usize >= MEMO_IDX_MAX_FUNS || key < 0 || key as usize >= MEMO_IDX_CAP
    {
        return;
    }
    let k = key as usize;
    MEMO_IDX.with(|t| {
        let mut tables = t.borrow_mut();
        let table = memo_idx_table(&mut tables, fun_id as usize);
        table.valid[k] = 1;
        table.values[k] = result;
    });
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_hits() -> i64 {
    MEMO_IDX.with(|t| {
        t.borrow()
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|x| x.hits as i64)
            .sum()
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_misses() -> i64 {
    MEMO_IDX.with(|t| {
        t.borrow()
            .iter()
            .filter_map(|x| x.as_ref())
            .map(|x| x.misses as i64)
            .sum()
    })
}

#[no_mangle]
pub extern "C" fn lumia_memo_idx_reset() {
    MEMO_IDX.with(|t| {
        *t.borrow_mut() = [const { None }; MEMO_IDX_MAX_FUNS];
    });
}

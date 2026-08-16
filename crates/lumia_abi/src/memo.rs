//! Transparent memo (`T_f`) caps and small-container threshold.

/// Transparent memo (`T_f`) hard caps — must stay in sync across opt planner and rt.
///
/// Naming: DESIGN / Rust use `T_f` / `MEMO_TF_*`. Exported C symbols stay
/// `lumia_memo_l2_*` — **`L2` is a historical ABI name, frozen; do not rename.**
pub const MEMO_TF_MAX_FUNS: usize = 64;
pub const MEMO_TF_SLOTS: usize = 4;
pub const MEMO_TF_MAX_ARGS: usize = 4;
pub const MEMO_PROCESS_BYTE_CAP: usize = 2 * 1024 * 1024;
pub const MEMO_IDX_MAX_FUNS: usize = 16;
/// Keys outside `0..MEMO_IDX_CAP` are never cached (DESIGN §7.5 hard bound).
pub const MEMO_IDX_CAP: usize = 4096;
pub const MEMO_IDX_TABLE_BYTES: usize = MEMO_IDX_CAP * (1 + 8);
pub const MEMO_SLOTS_TABLE_BYTES: usize = MEMO_TF_SLOTS * (1 + MEMO_TF_MAX_ARGS * 8 + 8);

/// Max elems / key–value pairs for Lit\* / Small\* stack layouts and linear
/// Map·Set before hash promote. Escape analysis, ReprSelect, and RT must share
/// this threshold (DESIGN §7 representation selection).
pub const SMALL_CONTAINER_MAX: usize = 8;

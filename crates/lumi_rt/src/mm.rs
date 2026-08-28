//! Memory manager mode selection (`LUMI_MM`, `lumi_set_mm_mode`).
//!
//! - **MarkSweep** (default): generational GC; COW `rc` is uniqueness only.
//! - **Arc**: all heap objects start at `rc=1`; COW + `lumi_heap_retain`/`release`
//!   free eagerly when `rc` hits 0. Full GC is STW (no concurrent mark) so
//!   free-on-zero cannot race the marker; cycles still use mark-sweep.

use std::cell::Cell;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmMode {
    MarkSweep = 0,
    /// Eager free-on-zero for COW types + mark-sweep cycle collection.
    Arc = 1,
}

thread_local! {
    static MM_MODE: Cell<MmMode> = const { Cell::new(MmMode::MarkSweep) };
}

static MM_ENV_INIT: OnceLock<()> = OnceLock::new();

fn parse_mm(s: &str) -> MmMode {
    match s.trim().to_ascii_lowercase().as_str() {
        "arc" | "refcount" => MmMode::Arc,
        _ => MmMode::MarkSweep,
    }
}

fn init_mm_from_env() {
    MM_ENV_INIT.get_or_init(|| {
        if let Ok(v) = std::env::var("LUMI_MM") {
            MM_MODE.with(|c| c.set(parse_mm(&v)));
        }
        if let Ok(v) = std::env::var("LUMI_GC_MARK_THREADS") {
            if let Ok(n) = v.parse::<usize>() {
                if n > 1 {
                    crate::gc::configure_mark_parallelism(n);
                }
            }
        }
    });
}

pub(crate) fn current_mm_mode() -> MmMode {
    init_mm_from_env();
    MM_MODE.with(|c| c.get())
}

/// Runtime hook for CLI `--mm` (0 = mark-sweep, 1 = arc).
#[no_mangle]
pub extern "C" fn lumi_set_mm_mode(mode: i64) {
    let m = if mode == 1 {
        MmMode::Arc
    } else {
        MmMode::MarkSweep
    };
    MM_MODE.with(|c| c.set(m));
}

#[no_mangle]
pub extern "C" fn lumi_mm_mode() -> i64 {
    current_mm_mode() as i64
}

use lumia_core::{
    collect_assigns, collect_leaf_defs as core_collect_leaf_defs, for_each_block_dfs,
    header_ge_const, header_gt_const, is_unit_inc, Block, Op, Value,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Slots that are only ever assigned `0` or `self + 1` (Collatz `steps`, etc.).
pub(super) fn collect_unit_counter_slots(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
) -> HashSet<String> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);
    let mut out = HashSet::default();
    for (name, vals) in assigns {
        let mut has_zero = false;
        let mut ok = !vals.is_empty();
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(0)) => has_zero = true,
                _ if is_unit_inc(v.0, name.as_str(), all_defs) => {}
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && has_zero {
            out.insert(name);
        }
    }
    out
}

pub(crate) fn collect_safe_divisor_locals(body: &Block) -> HashSet<u32> {
    let all_defs = core_collect_leaf_defs(body, false);
    // ≥1 unit slots (init ≥1, only +=1) never hit 0/−1 — safe for udiv/urem.
    let ge1_slots = collect_ge1_unit_slots(body, &all_defs);
    let mut out = HashSet::default();
    for (id, value) in &all_defs {
        match value {
            Value::Int(n) if *n != 0 && *n != -1 => {
                out.insert(*id);
            }
            Value::Name(n) if ge1_slots.contains(n) => {
                out.insert(*id);
            }
            _ => {}
        }
    }
    out
}

/// Locals that are `Name(iv)` loads inside a loop whose header proves `iv >= 0`
/// (strict `iv > k` with `k >= -1`, or `iv >= k` with `k >= 0`, or counting-up
/// upper-bound `iv < n` / `iv <= n` with nonnegative init + unit incs only).
pub(crate) fn collect_nonneg_iv_load_locals(body: &Block) -> HashSet<u32> {
    let all_defs = core_collect_leaf_defs(body, false);
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);
    let mut out = HashSet::default();
    for_each_block_dfs(body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value:
                    Value::Loop {
                        header,
                        body: loop_body,
                        latch,
                    },
                ..
            } = op
            {
                for iv in nonneg_iv_names(header, &assigns, &all_defs) {
                    mark_name_loads(loop_body, iv.as_str(), &mut out);
                    mark_name_loads(latch, iv.as_str(), &mut out);
                }
            }
        }
    });
    out
}

fn nonneg_iv_names(
    header: &Block,
    assigns: &HashMap<String, Vec<lumia_core::Local>>,
    all_defs: &HashMap<u32, Value>,
) -> HashSet<String> {
    let mut names = HashSet::default();
    // `iv > k` / `k < iv` with k ≥ -1 ⇒ iv ≥ 0.
    if let Some((iv, k)) = header_gt_const(header, all_defs) {
        if k >= -1 {
            names.insert(iv);
        }
    }
    // `iv >= k` (IV on the left) with k ≥ 0.
    if let Some((iv, k)) = header_ge_const(header, all_defs) {
        if k >= 0 {
            names.insert(iv);
        }
    }
    // Counting-up upper bound (`i < n` / `i <= n` / `n > i` / …): IV stays ≥ 0
    // when init is a nonnegative const and other assigns are only unit +1.
    let info = super::bounds::iv_bound_info(header, all_defs);
    if info.is_upper {
        for iv in &info.ivs {
            if slot_nonneg_unit_up(assigns, iv, all_defs) {
                names.insert(iv.clone());
            }
        }
    }
    names
}

/// Slot init ≥ 0 (some `Int ≥ 0` assign) and every other assign is `self + 1`.
fn slot_nonneg_unit_up(
    assigns: &HashMap<String, Vec<lumia_core::Local>>,
    name: &str,
    all_defs: &HashMap<u32, Value>,
) -> bool {
    let Some(vals) = assigns.get(name) else {
        return false;
    };
    let mut has_nonneg_const = false;
    if vals.is_empty() {
        return false;
    }
    for v in vals {
        match all_defs.get(&v.0) {
            Some(Value::Int(n)) if *n >= 0 => has_nonneg_const = true,
            Some(Value::Int(_)) => return false,
            _ if is_unit_inc(v.0, name, all_defs) => {}
            _ => return false,
        }
    }
    has_nonneg_const
}

fn mark_name_loads(block: &Block, iv: &str, out: &mut HashSet<u32>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                local,
                value: Value::Name(n),
                ..
            } = op
            {
                if n == iv {
                    out.insert(local.0);
                }
            }
        }
    });
}

/// Mutable slots whose every assignment is `≥ 1` or `slot = slot + 1`.
///
/// Superset of [`collect_ge2_unit_slots`]; used for safe-divisor Name loads
/// (`n/i` with `i` from 1 is never ÷0).
pub(crate) fn collect_ge1_unit_slots(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
) -> HashSet<String> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);

    let mut ge1 = HashSet::default();
    for (name, vals) in assigns {
        let mut has_ge1_const = false;
        let mut ok = !vals.is_empty();
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(n)) if *n >= 1 => has_ge1_const = true,
                _ if is_unit_inc(v.0, name.as_str(), all_defs) => {}
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && has_ge1_const {
            ge1.insert(name);
        }
    }
    ge1
}

/// Mutable slots whose every assignment is `≥ 2` or `slot = slot + 1`.
/// Stricter than [`collect_ge1_unit_slots`] (start ≥ 2). Used by nsw_iv tests.
#[allow(dead_code)] // exercised from `nsw_iv_tests`
pub(crate) fn collect_ge2_unit_slots(
    body: &Block,
    all_defs: &HashMap<u32, Value>,
) -> HashSet<String> {
    let mut assigns = HashMap::default();
    collect_assigns(body, &mut assigns);

    let mut ge2 = HashSet::default();
    for (name, vals) in assigns {
        let mut has_ge2_const = false;
        let mut ok = !vals.is_empty();
        for v in vals {
            match all_defs.get(&v.0) {
                Some(Value::Int(n)) if *n >= 2 => has_ge2_const = true,
                _ if is_unit_inc(v.0, name.as_str(), all_defs) => {}
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && has_ge2_const {
            ge2.insert(name);
        }
    }
    ge2
}

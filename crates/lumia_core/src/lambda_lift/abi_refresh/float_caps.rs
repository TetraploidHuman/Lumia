//! Scan AllocClosure captures for Float slots (typed locals after mono).
//!
//! ABI comes from typed cap tables (`collect_fun_cap_tys` / codegen
//! `closure_cap_tys`). Fixup only seeds float locals so param/ret refresh still
//! sees nested `{ x -> x + k }` in `make$Float`.

use crate::ir::{Block, Local, Op, Value};
use crate::value_ty::{infer_value_ty_ctx, InferValueCtx};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

pub(super) fn scan_alloc_closure_caps(
    block: &Block,
    local_tys: &mut HashMap<u32, Type>,
    slot_tys: &mut HashMap<String, Type>,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    need_float: &mut HashSet<(String, u32)>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                note_alloc_caps(value, local_tys, need_float);
                let ty = infer_value_ty_ctx(
                    value,
                    InferValueCtx::with_fun_abi(
                        local_tys,
                        Some(slot_tys),
                        fun_ret_tys,
                        fun_param_tys,
                    ),
                    None,
                );
                local_tys.insert(local.0, ty);
                crate::for_each_nested_block(value, &mut |b| {
                    scan_alloc_closure_caps(
                        b,
                        local_tys,
                        slot_tys,
                        fun_ret_tys,
                        fun_param_tys,
                        need_float,
                    );
                });
            }
            Op::Assign { name, value } => {
                if let Some(ty) = local_tys.get(&value.0).cloned() {
                    slot_tys.insert(name.clone(), ty);
                }
            }
            Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
}

pub(super) fn note_alloc_caps(
    value: &Value,
    local_tys: &HashMap<u32, Type>,
    need_float: &mut HashSet<(String, u32)>,
) {
    if let Value::AllocClosure { fun, captures } = value {
        for (i, cap) in captures.iter().enumerate() {
            if matches!(local_tys.get(&cap.0), Some(Type::Float)) {
                need_float.insert((fun.name.clone(), i as u32));
            }
        }
    }
}

/// Locals bound to `ClosureCap` at `indices` — seed for float ABI refresh.
pub(super) fn seed_float_locals_from_cap_indices(
    block: &Block,
    indices: &HashSet<u32>,
) -> HashSet<u32> {
    let mut seed = HashSet::default();
    crate::visit::for_each_let(block, &mut |_b, local: Local, value| {
        if let Value::ClosureCap { index, .. } = value {
            if indices.contains(index) {
                seed.insert(local.0);
            }
        }
    });
    seed
}

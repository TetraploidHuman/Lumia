//! FunRef / IndirectCall → direct `Call` (spawn thunks, nested If/Loop).

use crate::ir::{Block, CoreModule, Op, Value};
use crate::visit::{collect_closure_cap_funrefs, for_each_top_level_op_in_block_mut};
use rustc_hash::{FxHashMap as HashMap, FxHashSet};
use lumia_hir::Sym;

pub(crate) fn directize_funref_calls(module: &mut CoreModule) {
    // AllocClosure capture index → FunRef name, so spawn thunks that capture
    // a FunRef still directize `icall` → `Call` (mono can specialize Float).
    // Do **not** directize when the captured value is itself a closure with an
    // env (`{ x -> g(x) }` under spawn): `Call(__lam_env, [x])` drops the env.
    let mut cap_funs: HashMap<String, HashMap<u32, String>> = HashMap::default();
    for fun in &module.functions {
        let mut funref_locals: HashMap<u32, String> = HashMap::default();
        collect_closure_cap_funrefs(&fun.body, &mut funref_locals, &mut cap_funs);
    }
    let with_env = funs_with_closure_env(module);
    let empty_funrefs = HashMap::default();
    let empty_slots = HashMap::default();
    for fun in &mut module.functions {
        let caps = cap_funs
            .get(fun.name.as_str())
            .cloned()
            .unwrap_or_default();
        let caps: HashMap<u32, String> = caps
            .into_iter()
            .filter(|(_, name)| !with_env.contains(name))
            .collect();
        directize_block_with_slots(&mut fun.body, &empty_funrefs, &empty_slots, &caps);
    }
}

/// Names of `__lam_*` / funs that are allocated with a non-empty capture list
/// (first param is the env pointer; must stay `IndirectCall`).
fn funs_with_closure_env(module: &CoreModule) -> FxHashSet<String> {
    let mut out = FxHashSet::default();
    for fun in &module.functions {
        crate::collect_alloc_closure_env_funs(&fun.body, &mut out);
    }
    out
}

pub(crate) fn directize_block(block: &mut Block, parent_funrefs: &HashMap<u32, String>) {
    directize_block_with_slots(
        block,
        parent_funrefs,
        &HashMap::default(),
        &HashMap::default(),
    );
}

fn directize_block_with_slots(
    block: &mut Block,
    parent_funrefs: &HashMap<u32, String>,
    parent_slot_funrefs: &HashMap<Sym, String>,
    cap_funs: &HashMap<u32, String>,
) {
    // Inherit FunRef bindings from the enclosing block so `val f = g; if … { f(x) }`
    // inside nested If/Loop still becomes a direct `Call`.
    let mut aliases = crate::FunRefAliases {
        locals: parent_funrefs.clone(),
        slots: parent_slot_funrefs.clone(),
    };
    for_each_top_level_op_in_block_mut(block, &mut |op| match op {
        Op::Let { local, value, .. } => {
            directize_value(value, &aliases.locals);
            walk_nested_blocks_directize(value, &aliases.locals, &aliases.slots, cap_funs);
            aliases.note_let(local.0, value, crate::FunRefAlloc::Ignore, Some(cap_funs));
        }
        Op::Assign { name, value } => aliases.note_assign(name.clone(), *value),
        Op::Break | Op::Continue | Op::Return { .. } => {}
    });
}

fn walk_nested_blocks_directize(
    value: &mut Value,
    funref_of: &HashMap<u32, String>,
    slot_funrefs: &HashMap<Sym, String>,
    cap_funs: &HashMap<u32, String>,
) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            directize_block_with_slots(then_block, funref_of, slot_funrefs, cap_funs);
            directize_block_with_slots(else_block, funref_of, slot_funrefs, cap_funs);
        }
        Value::Loop {
            header,
            body,
            latch,
            ..
        } => {
            directize_block_with_slots(header, funref_of, slot_funrefs, cap_funs);
            directize_block_with_slots(body, funref_of, slot_funrefs, cap_funs);
            directize_block_with_slots(latch, funref_of, slot_funrefs, cap_funs);
        }
        // Fresh scope: lifted lambda body should not see outer SSA FunRef locals.
        Value::Lambda { body, .. } => directize_block_with_slots(
            body,
            &HashMap::default(),
            &HashMap::default(),
            &HashMap::default(),
        ),
        _ => {}
    }
}

fn directize_value(value: &mut Value, funref_of: &HashMap<u32, String>) {
    let Value::IndirectCall { callee, args } = value else {
        return;
    };
    let Some(name) = funref_of.get(&callee.0) else {
        return;
    };
    let args = std::mem::take(args);
    *value = Value::Call {
        fun: name.clone().into(),
        args,
    };
}

#[cfg(test)]
#[path = "directize_tests.rs"]
mod tests;

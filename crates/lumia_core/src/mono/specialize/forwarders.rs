use crate::ir::{Block, CoreFun, CoreModule, Op, Value};
use crate::visit::{for_each_top_level_op_in_block, for_each_top_level_op_in_block_mut};
use rustc_hash::FxHashMap as HashMap;

/// Mono FunRef toehold: if a clone's result is `Call(target, params)` (pure
/// forwarder), rewrite call sites to `target` so bodies are shared in practice.
pub(super) fn elide_trivial_mono_forwarders(module: &mut CoreModule) {
    let mut forward: HashMap<String, String> = HashMap::default();
    for f in &module.functions {
        if let Some(target) = trivial_param_forward_target(f) {
            if target != f.name {
                forward.insert(f.name.to_string(), target);
            }
        }
    }
    if forward.is_empty() {
        return;
    }
    // Collapse chains A→B→C.
    let keys: Vec<String> = forward.keys().cloned().collect();
    for k in keys {
        let mut cur = forward.get(&k).cloned();
        let mut guard = 0;
        while let Some(ref t) = cur {
            if let Some(next) = forward.get(t) {
                cur = Some(next.clone());
                guard += 1;
                if guard > 8 {
                    break;
                }
            } else {
                break;
            }
        }
        if let Some(t) = cur {
            forward.insert(k, t);
        }
    }
    for fun in &mut module.functions {
        rewrite_forward_calls(&mut fun.body, &forward);
    }
}

fn trivial_param_forward_target(fun: &CoreFun) -> Option<String> {
    fun.mono_of.as_ref()?;
    let result = fun.body.result?;
    let mut target = None;
    for_each_top_level_op_in_block(&fun.body, &mut |op| {
        let Op::Let {
            local,
            value: Value::Call { fun: callee, args },
            ..
        } = op
        else {
            return;
        };
        if *local != result {
            return;
        }
        // Exact forward of all formals (identity-shaped mono clone).
        if args.len() == fun.params.len() && args.iter().eq(fun.params.iter()) {
            target = Some(callee.name.clone());
        }
    });
    target
}

fn rewrite_forward_calls(block: &mut Block, forward: &HashMap<String, String>) {
    for_each_top_level_op_in_block_mut(block, &mut |op| {
        if let Op::Let { value, .. } = op {
            rewrite_forward_value(value, forward);
        }
    });
}

fn rewrite_forward_value(value: &mut Value, forward: &HashMap<String, String>) {
    match value {
        Value::Call { fun, .. } => {
            if let Some(t) = forward.get(fun.as_str()) {
                *fun = t.clone().into();
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_forward_calls(then_block, forward);
            rewrite_forward_calls(else_block, forward);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_forward_calls(header, forward);
            rewrite_forward_calls(body, forward);
            rewrite_forward_calls(latch, forward);
        }
        Value::Lambda { body, .. } => rewrite_forward_calls(body, forward),
        _ => {}
    }
}

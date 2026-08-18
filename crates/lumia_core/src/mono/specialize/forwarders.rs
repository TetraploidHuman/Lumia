use crate::ir::{Block, CoreFun, CoreModule, Op, Value};
use rustc_hash::FxHashMap as HashMap;

/// Mono FunRef toehold: if a clone's result is `Call(target, params)` (pure
/// forwarder), rewrite call sites to `target` so bodies are shared in practice.
pub(super) fn elide_trivial_mono_forwarders(module: &mut CoreModule) {
    let mut forward: HashMap<String, String> = HashMap::default();
    for f in &module.functions {
        if let Some(target) = trivial_param_forward_target(f) {
            if target != f.name {
                forward.insert(f.name.clone(), target);
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
    for op in &fun.body.ops {
        let Op::Let {
            local,
            value: Value::Call { fun: target, args },
            ..
        } = op
        else {
            continue;
        };
        if *local != result {
            continue;
        }
        // Exact forward of all formals (identity-shaped mono clone).
        if args.len() == fun.params.len() && args.iter().eq(fun.params.iter()) {
            return Some(target.name.clone());
        }
    }
    None
}

fn rewrite_forward_calls(block: &mut Block, forward: &HashMap<String, String>) {
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } => {
                rewrite_forward_value(value, forward);
            }
            _ => {}
        }
    }
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

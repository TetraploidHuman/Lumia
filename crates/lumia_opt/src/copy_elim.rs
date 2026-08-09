//! Copy elimination: collapse `let x = y` aliases (SSA copy-prop).

use crate::Pass;
use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Copy elimination: collapse `let x = y` aliases (SSA copy-prop).
pub(crate) struct CopyElimPass;
impl Pass for CopyElimPass {
    fn name(&self) -> &str {
        "copy_elim"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            elim_copies_in_fun(f);
        }
    }
}

fn elim_copies_in_fun(f: &mut CoreFun) {
    let mut remap: HashMap<u32, u32> = HashMap::default();
    collect_copy_aliases(&f.body, &mut remap);
    if remap.is_empty() {
        return;
    }
    // Flatten chains a→b→c to a→c.
    let keys: Vec<u32> = remap.keys().copied().collect();
    for k in keys {
        let mut cur = k;
        let mut seen = HashSet::default();
        while let Some(&n) = remap.get(&cur) {
            if !seen.insert(cur) {
                break;
            }
            cur = n;
        }
        remap.insert(k, cur);
    }
    apply_local_remap(&mut f.body, &remap);
    strip_identity_lets(&mut f.body, &remap);
}

fn collect_copy_aliases(block: &Block, remap: &mut HashMap<u32, u32>) {
    for op in &block.ops {
        match op {
            Op::Let {
                local,
                value: Value::Local(src),
                ..
            } => {
                remap.insert(local.0, src.0);
            }
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                collect_copy_aliases_value(value, remap);
            }
            _ => {}
        }
    }
}

fn collect_copy_aliases_value(value: &Value, remap: &mut HashMap<u32, u32>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_copy_aliases(then_block, remap);
            collect_copy_aliases(else_block, remap);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_copy_aliases(header, remap);
            collect_copy_aliases(body, remap);
            collect_copy_aliases(latch, remap);
        }
        Value::Lambda { body, .. } => collect_copy_aliases(body, remap),
        _ => {}
    }
}

fn apply_local_remap(block: &mut Block, remap: &HashMap<u32, u32>) {
    let map_l = |l: &mut Local| {
        if let Some(&r) = remap.get(&l.0) {
            *l = Local(r);
        }
    };
    if let Some(r) = &mut block.result {
        map_l(r);
    }
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                remap_value_locals(value, remap);
            }
            Op::Assign { value, .. } | Op::Return { value } => map_l(value),
            Op::Break | Op::Continue => {}
        }
    }
}

fn remap_value_locals(value: &mut Value, remap: &HashMap<u32, u32>) {
    lumia_core::map_value_locals(
        value,
        &mut |l| {
            if let Some(&r) = remap.get(&l.0) {
                *l = Local(r);
            }
        },
        &mut |b| apply_local_remap(b, remap),
    );
}

fn strip_identity_lets(block: &mut Block, aliases: &HashMap<u32, u32>) {
    block.ops.retain(|op| {
        !matches!(
            op,
            Op::Let { local, .. } if aliases.contains_key(&local.0)
        )
    });
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                strip_identity_lets_value(value, aliases);
            }
            _ => {}
        }
    }
}

fn strip_identity_lets_value(value: &mut Value, aliases: &HashMap<u32, u32>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            strip_identity_lets(then_block, aliases);
            strip_identity_lets(else_block, aliases);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            strip_identity_lets(header, aliases);
            strip_identity_lets(body, aliases);
            strip_identity_lets(latch, aliases);
        }
        Value::Lambda { body, .. } => strip_identity_lets(body, aliases),
        _ => {}
    }
}

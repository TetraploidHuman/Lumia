//! Copy elimination: collapse `let x = y` aliases (SSA copy-prop).

use lumia_core::{
    for_each_block_dfs, for_each_nested_block_mut, Block, CoreFun, CoreModule, Local, Op, Value,
};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Copy elimination: collapse `let x = y` aliases (SSA copy-prop).
pub(crate) struct CopyElimPass;
impl CopyElimPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
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
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                local,
                value: Value::Local(src),
                ..
            } = op
            {
                remap.insert(local.0, src.0);
            }
        }
    });
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
            Op::Let { value, .. } => {
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
            Op::Assign { value, .. } | Op::Return { value } => map_l(value),
            Op::Break | Op::Continue => {}
        }
    }
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
            Op::Let { value, .. } => {
                for_each_nested_block_mut(value, &mut |nested| {
                    strip_identity_lets(nested, aliases);
                });
            }
            _ => {}
        }
    }
}

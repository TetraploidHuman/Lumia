//! Residual `List.concat` identity elimination after HIR fusion (DESIGN §7.2).
//!
//! Primary fusion of `map`/`filter`/`fold` runs in HIR (`try_fuse_hof_fold`).
//! This Core pass only peels `xs.concat([])` / `[].concat(xs)` → `xs`.
//! Build-side deforestation (`flatMap` materialize, fused views) is not here.

use lumia_core::{
    for_each_block_dfs, for_each_op_value_mut, Block, CoreFun, CoreModule, Local, Op, Value,
};
use lumia_hir::Builtin;
use rustc_hash::FxHashSet as HashSet;

/// Peel empty-list concat identities left after HIR deforestation.
pub struct ConcatIdentPass;

impl ConcatIdentPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        for f in &mut module.functions {
            fuse_fun(f);
        }
    }
}

fn fuse_fun(f: &mut CoreFun) {
    let mut empty_lists: HashSet<u32> = HashSet::default();
    collect_empty_lists(&f.body, &mut empty_lists);
    if empty_lists.is_empty() {
        return;
    }
    rewrite_block(&mut f.body, &empty_lists);
}

fn collect_empty_lists(block: &Block, empty: &mut HashSet<u32>) {
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                local,
                value: Value::AllocList { elems, .. },
                ..
            } = op
            {
                if elems.is_empty() {
                    empty.insert(local.0);
                }
            }
        }
    });
}

fn rewrite_block(block: &mut Block, empty: &HashSet<u32>) {
    for_each_op_value_mut(block, &mut |value| {
        if let Value::Builtin {
            name: Builtin::ListConcat,
            args,
            ..
        } = value
        {
            if args.len() == 2 {
                let a = args[0].0;
                let b = args[1].0;
                if empty.contains(&a) {
                    *value = Value::Local(Local(b));
                } else if empty.contains(&b) {
                    *value = Value::Local(Local(a));
                }
            }
        }
    });
}

#[cfg(test)]
#[path = "concat_ident_tests.rs"]
mod tests;

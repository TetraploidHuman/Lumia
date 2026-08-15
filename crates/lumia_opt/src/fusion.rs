//! Residual `List.concat` identity elimination after HIR fusion (DESIGN §7.2).
//!
//! Primary fusion of `map`/`filter`/`fold` runs in HIR (`try_fuse_hof_fold` /
//! `try_fuse_hof_build_method`). This Core pass only peels
//! `xs.concat([])` / `[].concat(xs)` → `xs`.

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
            args, .. } = value
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
mod tests {
    use super::*;
        use lumia_core::{Block, CoreFun, CoreModule, ListRepr, Op, Value};
    use lumia_ty::{Effect, Type};
    use rustc_hash::FxHashSet as HashSet;

    #[test]
    fn peels_concat_with_empty() {
        let mut module = CoreModule::with_functions(
            "M",
            vec![CoreFun {
                name: "f".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    params: vec![],
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::AllocList {
                                elems: vec![],
                                repr: ListRepr::LitList,
                            },
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::AllocList {
                                elems: vec![Local(1)],
                                repr: ListRepr::HeapList,
                            },
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(3),
                            value: Value::Builtin {
                                name: Builtin::ListConcat,
                                args: vec![Local(0), Local(2)],
                    result_ty: None,
                },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(3)),
                },
                ret_ty: Type::List(Box::new(Type::Int)),
                effect: Effect::pure(),
                is_main: false,
                memo: None,
                external: None,
                foreign_abi: lumia_core::ForeignAbi::C,
                escaping: HashSet::default(),
                scheme_poly: false,
                mono_of: None,
            }],
        );
        ConcatIdentPass.run(&mut module);
        assert!(matches!(
            &module.functions[0].body.ops[3],
            Op::Let {
                value: Value::Local(Local(2)),
                ..
            }
        ));
    }
}

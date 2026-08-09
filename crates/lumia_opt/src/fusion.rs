//! Deforestation / pipeline fusion (DESIGN §7.2).
//!
//! Primary fusion of `map`/`filter`/`fold` runs in HIR (`try_fuse_hof_fold` /
//! `try_fuse_hof_build_method`). This Core pass peels residual identity
//! concatenations (`xs.concat([])` / `[].concat(xs)` → `xs`) after HIR.

use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use std::collections::HashSet;

pub struct FusionPass;

impl crate::Pass for FusionPass {
    fn name(&self) -> &str {
        "fusion"
    }
    fn run(&self, module: &mut CoreModule) {
        for f in &mut module.functions {
            fuse_fun(f);
        }
    }
}

fn fuse_fun(f: &mut CoreFun) {
    let mut empty_lists: HashSet<u32> = HashSet::new();
    collect_empty_lists(&f.body, &mut empty_lists);
    if empty_lists.is_empty() {
        return;
    }
    rewrite_block(&mut f.body, &empty_lists);
}

fn collect_empty_lists(block: &Block, empty: &mut HashSet<u32>) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                if matches!(value, Value::AllocList { elems, .. } if elems.is_empty()) {
                    empty.insert(local.0);
                }
                collect_empty_in_value(value, empty);
            }
            Op::Effect { value } => collect_empty_in_value(value, empty),
            _ => {}
        }
    }
}

fn collect_empty_in_value(value: &Value, empty: &mut HashSet<u32>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_empty_lists(then_block, empty);
            collect_empty_lists(else_block, empty);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_empty_lists(header, empty);
            collect_empty_lists(body, empty);
            collect_empty_lists(latch, empty);
        }
        Value::Lambda { body, .. } => collect_empty_lists(body, empty),
        _ => {}
    }
}

fn rewrite_block(block: &mut Block, empty: &HashSet<u32>) {
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                rewrite_value(value, empty);
            }
            _ => {}
        }
    }
}

fn rewrite_value(value: &mut Value, empty: &HashSet<u32>) {
    match value {
        Value::Builtin {
            name: Builtin::ListConcat,
            args,
        } if args.len() == 2 => {
            let a = args[0].0;
            let b = args[1].0;
            if empty.contains(&a) {
                *value = Value::Local(Local(b));
            } else if empty.contains(&b) {
                *value = Value::Local(Local(a));
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_block(then_block, empty);
            rewrite_block(else_block, empty);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_block(header, empty);
            rewrite_block(body, empty);
            rewrite_block(latch, empty);
        }
        Value::Lambda { body, .. } => rewrite_block(body, empty),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pass;
    use lumia_core::{Block, CoreFun, CoreModule, ListRepr, Op, Value};
    use lumia_ty::{Effect, Type};

    #[test]
    fn peels_concat_with_empty() {
        let mut module = CoreModule {
            name: "M".into(),
            functions: vec![CoreFun {
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
            escaping: std::collections::HashSet::new(),
            }],
            hash_adts: std::collections::HashSet::new(),
        trait_methods: std::collections::HashMap::new(),
        };
        FusionPass.run(&mut module);
        assert!(matches!(
            &module.functions[0].body.ops[3],
            Op::Let {
                value: Value::Local(Local(2)),
                ..
            }
        ));
    }
}

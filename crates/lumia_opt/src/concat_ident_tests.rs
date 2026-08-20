use super::*;
use lumia_core::{Block, CoreFun, CoreModule, FunKind, ListRepr, Op, Value};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;
use std::sync::Arc;

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
            ret_ty: Type::List(Arc::new(Type::Int)),
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
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

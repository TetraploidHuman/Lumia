    use super::*;
        use lumia_core::{Block, CoreFun, CoreModule, ListRepr, Op, Value, FunKind};
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

use super::*;

#[test]
fn const_fold_folds_list_len_get() {
    use lumia_core::ListRepr;
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(10),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(20),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::AllocList {
                            elems: vec![Local(0), Local(1)],
                            repr: ListRepr::LitList,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Builtin {
                            name: Builtin::ListLen,
                            args: vec![Local(2)],
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(5),
                        value: Value::Builtin {
                            name: Builtin::ListGet,
                            args: vec![Local(2), Local(4)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(5)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(matches!(
        &module.functions[0].body.ops[3],
        Op::Let {
            value: Value::Int(2),
            ..
        }
    ));
    assert!(matches!(
        &module.functions[0].body.ops[5],
        Op::Let {
            value: Value::Local(Local(1)),
            ..
        }
    ));
}

#[test]
fn const_fold_folds_list_concat() {
    use lumia_core::ListRepr;
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::AllocList {
                            elems: vec![Local(0)],
                            repr: ListRepr::LitList,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::AllocList {
                            elems: vec![Local(1)],
                            repr: ListRepr::LitList,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::Builtin {
                            name: Builtin::ListConcat,
                            args: vec![Local(2), Local(3)],
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(5),
                        value: Value::Builtin {
                            name: Builtin::ListLen,
                            args: vec![Local(4)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(5)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(
        matches!(
            &module.functions[0].body.ops[4],
            Op::Let {
                value: Value::AllocList {
                    elems,
                    repr: ListRepr::LitList
                },
                ..
            } if elems == &[Local(0), Local(1)]
        ),
        "ListConcat of lit lists should PE-fold, got {:?}",
        module.functions[0].body.ops[4]
    );
    assert!(matches!(
        &module.functions[0].body.ops[5],
        Op::Let {
            value: Value::Int(2),
            ..
        }
    ));
}

#[test]
fn const_fold_list_take_slice_reverse() {
    use lumia_core::ListRepr;
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Int(3),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::AllocList {
                            elems: vec![Local(0), Local(1), Local(2)],
                            repr: ListRepr::LitList,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(5),
                        value: Value::Builtin {
                            name: Builtin::ListTake,
                            args: vec![Local(3), Local(4)],
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(6),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(7),
                        value: Value::Builtin {
                            name: Builtin::ListSlice,
                            args: vec![Local(3), Local(6)],
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(8),
                        value: Value::Builtin {
                            name: Builtin::ListReverse,
                            args: vec![Local(3)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(8)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(
        matches!(
            &module.functions[0].body.ops[5],
            Op::Let {
                value: Value::AllocList { elems, .. },
                ..
            } if elems == &[Local(0), Local(1)]
        ),
        "ListTake should PE, got {:?}",
        module.functions[0].body.ops[5]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[7],
            Op::Let {
                value: Value::AllocList { elems, .. },
                ..
            } if elems == &[Local(1), Local(2)]
        ),
        "ListSlice/drop should PE, got {:?}",
        module.functions[0].body.ops[7]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[8],
            Op::Let {
                value: Value::AllocList { elems, .. },
                ..
            } if elems == &[Local(2), Local(1), Local(0)]
        ),
        "ListReverse should PE, got {:?}",
        module.functions[0].body.ops[8]
    );
}

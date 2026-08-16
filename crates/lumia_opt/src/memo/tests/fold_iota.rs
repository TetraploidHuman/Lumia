use super::*;

#[test]
fn const_fold_iota_len_get() {
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(10),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(13),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Builtin {
                            name: Builtin::Range,
                            args: vec![Local(0), Local(1)],
                    result_ty: None,
                },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Builtin {
                            name: Builtin::ListLen,
                            args: vec![Local(2)],
                    result_ty: None,
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
                    result_ty: None,
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
            value: Value::Int(3),
            ..
        }
    ));
    assert!(matches!(
        &module.functions[0].body.ops[5],
        Op::Let {
            value: Value::Int(11),
            ..
        }
    ));
}

use super::*;

#[test]
fn const_fold_arith() {
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
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(3),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Binary {
                            op: BinOp::Mul,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(2)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(matches!(
        &module.functions[0].body.ops[2],
        Op::Let {
            value: Value::Int(6),
            ..
        }
    ));
}

#[test]
fn const_fold_folds_cmp_to_bool() {
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
                        value: Value::Binary {
                            op: BinOp::Lt,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(2)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(matches!(
        &module.functions[0].body.ops[2],
        Op::Let {
            value: Value::Bool(true),
            ..
        }
    ));
}

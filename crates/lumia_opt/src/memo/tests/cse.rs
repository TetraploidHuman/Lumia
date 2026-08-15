use super::*;

#[test]
fn cse_dedups_int_and_nontrapping_binary() {
    // Add/Sub/Mul/Div/Rem are not CSE'd (may trap). Eq is pure and may share.
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "main",
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
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Binary {
                            op: BinOp::Eq,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Binary {
                            op: BinOp::Eq,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(3)),
            },
        )],
    );
    module.functions[0].is_main = true;
    module.functions[0].effect = Effect::io();
    cse_module(&mut module);
    let ops = &module.functions[0].body.ops;
    assert!(matches!(
        &ops[1],
        Op::Let {
            value: Value::Local(Local(0)),
            ..
        }
    ));
    assert!(matches!(
        &ops[3],
        Op::Let {
            value: Value::Local(_),
            ..
        }
    ));
}

#[test]
fn cse_preserves_distinct_external_calls() {
    let mut getpid = bare_fun(
        "getpid",
        vec![],
        Block {
            params: vec![],
            ops: vec![],
            result: None,
        },
    );
    getpid.external = Some("getpid".into());
    getpid.foreign_abi = lumia_core::ForeignAbi::C;
    getpid.effect = Effect::pure();
    let mut module = CoreModule::with_functions(
        "C",
        vec![
            getpid,
            bare_fun(
                "main",
                vec![],
                Block {
                    params: vec![],
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Call {
                                fun: "getpid".into(),
                                args: vec![],
                            },
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Call {
                                fun: "getpid".into(),
                                args: vec![],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(1)),
                },
            ),
        ],
    );
    module.functions[1].is_main = true;
    module.functions[1].effect = Effect::io();
    cse_module(&mut module);
    let ops = &module.functions[1].body.ops;
    assert!(
        matches!(
            &ops[0],
            Op::Let {
                value: Value::Call { fun, .. },
                ..
            } if fun == "getpid"
        ),
        "first foreign call must remain"
    );
    assert!(
        matches!(
            &ops[1],
            Op::Let {
                value: Value::Call { fun, .. },
                ..
            } if fun == "getpid"
        ),
        "second foreign call must not be CSE'd into the first"
    );
}

#[test]
fn cse_dedups_float_arith() {
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "main",
            vec![],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Float(1.5),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Float(2.5),
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
                    Op::Let {
                        local: Local(3),
                        value: Value::Binary {
                            op: BinOp::Mul,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(3)),
            },
        )],
    );
    module.functions[0].is_main = true;
    module.functions[0].effect = Effect::io();
    cse_module(&mut module);
    let ops = &module.functions[0].body.ops;
    assert!(
        matches!(
            &ops[3],
            Op::Let {
                value: Value::Local(Local(2)),
                ..
            }
        ),
        "duplicate Float Mul must CSE to Local(2): {ops:?}"
    );
}

#[test]
fn cse_does_not_dedup_int_mul() {
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "main",
            vec![],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(3),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(4),
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
                    Op::Let {
                        local: Local(3),
                        value: Value::Binary {
                            op: BinOp::Mul,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(3)),
            },
        )],
    );
    module.functions[0].is_main = true;
    module.functions[0].effect = Effect::io();
    cse_module(&mut module);
    let ops = &module.functions[0].body.ops;
    assert!(
        matches!(
            &ops[3],
            Op::Let {
                value: Value::Binary {
                    op: BinOp::Mul,
                    ..
                },
                ..
            }
        ),
        "Int Mul must not CSE (may trap): {ops:?}"
    );
}

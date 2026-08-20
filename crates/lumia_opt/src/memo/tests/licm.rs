use super::*;

#[test]
fn licm_hoists_not_but_not_trapping_add() {
    // Checked Add may trap — must stay in-loop (§2.4). Pure Bool `not` is safe to hoist.
    let mut module = CoreModule::with_functions(
        "L",
        vec![bare_fun(
            "f",
            vec![Local(0)],
            Block {
                ops: vec![Op::Let {
                    local: Local(10),
                    value: Value::Loop {
                        header: Box::new(Block {
                            ops: vec![Op::Let {
                                local: Local(2),
                                value: Value::Bool(true),
                                pure_region: true,
                            }],
                            result: Some(Local(2)),
                        }),
                        body: Box::new(Block {
                            ops: vec![
                                Op::Let {
                                    local: Local(3),
                                    value: Value::Binary {
                                        op: BinOp::Add,
                                        left: Local(0),
                                        right: Local(0),
                                    },
                                    pure_region: true,
                                },
                                Op::Let {
                                    local: Local(4),
                                    value: Value::Unary {
                                        op: UnOp::Not,
                                        operand: Local(0),
                                    },
                                    pure_region: true,
                                },
                            ],
                            result: Some(Local(4)),
                        }),
                        latch: Box::new(Block {
                            ops: vec![],
                            result: None,
                        }),
                    },
                    pure_region: true,
                }],
                result: Some(Local(10)),
            },
        )],
    );
    LicmPass.run(&mut module);
    let ops = &module.functions[0].body.ops;
    assert!(
        matches!(
            &ops[0],
            Op::Let {
                value: Value::Unary { op: UnOp::Not, .. },
                ..
            }
        ),
        "invariant `not` should hoist before loop, got {:?}",
        ops[0]
    );
    let body_ops = match &ops[1] {
        Op::Let {
            value: Value::Loop { body, .. },
            ..
        } => &body.ops,
        other => panic!("expected loop as second op, got {other:?}"),
    };
    assert!(
        body_ops.iter().any(|op| matches!(
            op,
            Op::Let {
                value: Value::Binary { op: BinOp::Add, .. },
                ..
            }
        )),
        "trapping Add must remain inside the loop"
    );
}

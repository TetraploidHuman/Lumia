use super::*;
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{Block, CoreFun, CoreModule, FunKind, Local, Op, Value};
use lumia_ty::Effect;

fn bare_fun(name: &str, ops: Vec<Op>, result: Option<Local>) -> CoreFun {
    CoreFun {
        name: name.into(),
        params: vec![],
        param_names: vec![],
        param_tys: vec![],
        ret_ty: lumia_ty::Type::Int,
        effect: Effect::pure(),
        body: Block { ops, result },
        is_main: false,
        memo: None,
        external: None,
        foreign_abi: lumia_core::ForeignAbi::C,
        escaping: Default::default(),
        nsw_binop_locals: Default::default(),
        safe_divisor_locals: Default::default(),
        nonneg_iv_load_locals: Default::default(),
        scheme_poly: false,
        mono_of: None,
        kind: FunKind::Normal,
    }
}

#[test]
fn drops_unused_pure_literal() {
    let mut module = CoreModule::with_functions(
        "D",
        vec![bare_fun(
            "f",
            vec![
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
            ],
            Some(Local(1)),
        )],
    );
    DcePass.run(&mut module);
    let ops = &module.functions[0].body.ops;
    assert_eq!(ops.len(), 1);
    assert!(matches!(
        &ops[0],
        Op::Let {
            local: Local(1),
            ..
        }
    ));
}

#[test]
fn keeps_unused_int_div() {
    let mut module = CoreModule::with_functions(
        "D",
        vec![bare_fun(
            "f",
            vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Int(0),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Binary {
                        op: BinOp::Div,
                        left: Local(0),
                        right: Local(1),
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Int(9),
                    pure_region: true,
                },
            ],
            Some(Local(3)),
        )],
    );
    DcePass.run(&mut module);
    let ops = &module.functions[0].body.ops;
    assert!(
        ops.iter().any(|op| matches!(
            op,
            Op::Let {
                value: Value::Binary { op: BinOp::Div, .. },
                ..
            }
        )),
        "unused Int Div must remain (may trap): {ops:?}"
    );
}

#[test]
fn keeps_temp_only_used_inside_loop() {
    // `%0 = 0` assigned into a slot read only in the loop body.
    let loop_body = Block {
        ops: vec![Op::Assign {
            name: "acc".into(),
            value: Local(0),
        }],
        result: None,
    };
    let mut module = CoreModule::with_functions(
        "D",
        vec![bare_fun(
            "f",
            vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(0),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Loop {
                        header: Box::new(Block {
                            ops: vec![],
                            result: Some(Local(0)),
                        }),
                        body: Box::new(loop_body),
                        latch: Box::new(Block {
                            ops: vec![],
                            result: None,
                        }),
                    },
                    pure_region: false,
                },
            ],
            Some(Local(1)),
        )],
    );
    DcePass.run(&mut module);
    let ops = &module.functions[0].body.ops;
    assert!(
        ops.iter().any(|op| matches!(
            op,
            Op::Let {
                local: Local(0),
                value: Value::Int(0),
                ..
            }
        )),
        "loop-only temp must survive: {ops:?}"
    );
}

#[test]
fn drops_unused_range_and_adt_tag() {
    use lumia_hir::Builtin;
    let mut module = CoreModule::with_functions(
        "D",
        vec![bare_fun(
            "f",
            vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(0),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Int(3),
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
                        name: Builtin::AdtTag,
                        args: vec![Local(0)],
                        result_ty: None,
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(4),
                    value: Value::Int(9),
                    pure_region: true,
                },
            ],
            Some(Local(4)),
        )],
    );
    DcePass.run(&mut module);
    let ops = &module.functions[0].body.ops;
    assert!(
        !ops.iter().any(|op| matches!(
            op,
            Op::Let {
                value: Value::Builtin {
                    name: Builtin::Range | Builtin::AdtTag,
                    ..
                },
                ..
            }
        )),
        "unused pure Range/AdtTag must drop: {ops:?}"
    );
    assert_eq!(
        ops.len(),
        1,
        "dead Int args of dropped builtins must go: {ops:?}"
    );
    assert!(
        matches!(
            &ops[0],
            Op::Let {
                local: Local(4),
                value: Value::Int(9),
                ..
            }
        ),
        "result Int must remain: {ops:?}"
    );
}

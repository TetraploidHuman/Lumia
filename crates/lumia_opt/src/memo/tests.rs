use super::*;
use crate::Pass;
use lumia_core::{Block, CoreFun, CoreModule, Local, MemoTf, Op, Value};
use lumia_hir::Builtin;
use lumia_syntax::{BinOp, UnOp};
use lumia_ty::{Effect, Type};

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
fn bare_fun(name: &str, params: Vec<Local>, body: Block) -> CoreFun {
    let n = params.len();
    CoreFun {
        name: name.into(),
        params,
        param_names: (0..n).map(|i| format!("p{i}")).collect(),
        param_tys: vec![Type::Int; n],
        body,
        ret_ty: Type::Int,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: None,
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
    }
}

#[test]
fn cse_dedups_int_and_nontrapping_binary() {
    // Add/Sub/Mul/Div/Rem are not CSE'd (may trap). Eq is pure and may share.
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![bare_fun(
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
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
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
    getpid.effect = Effect::pure();
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![
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
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
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
fn const_fold_folds_list_len_get() {
    use lumia_core::ListRepr;
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![bare_fun(
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
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
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
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![bare_fun(
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
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
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
fn const_fold_map_get_to_option() {
    use lumia_core::{AdtRepr, MapRepr};
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![bare_fun(
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
                        value: Value::Int(10),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Int(20),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::AllocMap {
                            flat_pairs: vec![Local(0), Local(1), Local(2), Local(3)],
                            repr: MapRepr::LitMap,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(5),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(6),
                        value: Value::Builtin {
                            name: Builtin::ListGet,
                            args: vec![Local(4), Local(5)],
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(7),
                        value: Value::Int(9),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(8),
                        value: Value::Builtin {
                            name: Builtin::ListGet,
                            args: vec![Local(4), Local(7)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(6)),
            },
        )],
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
    ConstFoldPass.run(&mut module);
    assert!(
        matches!(
            &module.functions[0].body.ops[6],
            Op::Let {
                value: Value::AllocAdt {
                    adt_name,
                    tag: 0,
                    fields,
                    repr: AdtRepr::LitAdt,
                },
                ..
            } if adt_name == "Option" && fields == &[Local(3)]
        ),
        "map.get(hit) should PE to Some, got {:?}",
        module.functions[0].body.ops[6]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[8],
            Op::Let {
                value: Value::AllocAdt {
                    adt_name,
                    tag: 1,
                    fields,
                    repr: AdtRepr::LitAdt,
                },
                ..
            } if adt_name == "Option" && fields.is_empty()
        ),
        "map.get(miss) should PE to None, got {:?}",
        module.functions[0].body.ops[8]
    );
}

#[test]
fn const_fold_contains_skips_nonconst_keys() {
    // mapOf(nonconst_key to 2).contains(1) must not fold to false.
    use lumia_core::MapRepr;
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![bare_fun(
            "f",
            vec![Local(0)],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::AllocMap {
                            flat_pairs: vec![Local(0), Local(1)],
                            repr: MapRepr::HashOrdered,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::Builtin {
                            name: Builtin::Contains,
                            args: vec![Local(2), Local(3)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(4)),
            },
        )],
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
    ConstFoldPass.run(&mut module);
    assert!(
        matches!(
            &module.functions[0].body.ops[3],
            Op::Let {
                value: Value::Builtin {
                    name: Builtin::Contains,
                    ..
                },
                ..
            }
        ),
        "non-constant map key must not PE-fold contains, got {:?}",
        module.functions[0].body.ops[3]
    );
}

#[test]
fn const_fold_arith() {
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![bare_fun(
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
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
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
    let mut module = CoreModule {
        name: "C".into(),
        functions: vec![bare_fun(
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
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
    ConstFoldPass.run(&mut module);
    assert!(matches!(
        &module.functions[0].body.ops[2],
        Op::Let {
            value: Value::Bool(true),
            ..
        }
    ));
}

#[test]
fn licm_hoists_not_but_not_trapping_add() {
    // Checked Add may trap — must stay in-loop (§2.4). Pure Bool `not` is safe to hoist.
    let mut module = CoreModule {
        name: "L".into(),
        functions: vec![bare_fun(
            "f",
            vec![Local(0)],
            Block {
                params: vec![],
                ops: vec![Op::Let {
                    local: Local(10),
                    value: Value::Loop {
                        header: Box::new(Block {
                            params: vec![],
                            ops: vec![Op::Let {
                                local: Local(2),
                                value: Value::Bool(true),
                                pure_region: true,
                            }],
                            result: Some(Local(2)),
                        }),
                        body: Box::new(Block {
                            params: vec![],
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
                            params: vec![],
                            ops: vec![],
                            result: None,
                        }),
                    },
                    pure_region: true,
                }],
                result: Some(Local(10)),
            },
        )],
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
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

#[test]
fn memo_tf_marks_dense_int() {
    // fib-like: f(n) = f(n-1) with enough body weight.
    let mut fib = bare_fun(
        "fib",
        vec![Local(0)],
        Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Binary {
                        op: BinOp::Sub,
                        left: Local(0),
                        right: Local(1),
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Call {
                        fun: "fib".into(),
                        args: vec![Local(2)],
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(4),
                    value: Value::Binary {
                        op: BinOp::Add,
                        left: Local(3),
                        right: Local(1),
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(4)),
        },
    );
    fib.param_names = vec!["n".into()];
    let module = CoreModule {
        name: "M".into(),
        functions: vec![fib],
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
    let plan = plan_memo_tf(&module);
    assert!(
        matches!(plan.get("fib"), Some(MemoTf::DenseInt { .. })),
        "expected DenseInt, got {:?}",
        plan.get("fib")
    );
}

#[test]
fn memo_tf_marks_slots() {
    // Pure multi-arg with static same-arg reuse from caller → Slots.
    let mut sq = bare_fun(
        "sq",
        vec![Local(0)],
        Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Binary {
                        op: BinOp::Mul,
                        left: Local(0),
                        right: Local(0),
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Binary {
                        op: BinOp::Add,
                        left: Local(1),
                        right: Local(0),
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Binary {
                        op: BinOp::Mul,
                        left: Local(2),
                        right: Local(2),
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(3)),
        },
    );
    sq.param_names = vec!["n".into()];
    let main = CoreFun {
        name: "main".into(),
        params: vec![],
        param_names: vec![],
        param_tys: vec![],
        body: Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(99),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Call {
                        fun: "sq".into(),
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Call {
                        fun: "sq".into(),
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        },
        ret_ty: Type::Int,
        effect: Effect::io(),
        is_main: true,
        memo: None,
        external: None,
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
    };
    let module = CoreModule {
        name: "M".into(),
        functions: vec![sq, main],
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
    let plan = plan_memo_tf(&module);
    assert!(
        matches!(plan.get("sq"), Some(MemoTf::Slots { .. })),
        "expected Slots, got {:?}",
        plan.get("sq")
    );
}

#[test]
fn memo_tf_increasing_recursion_not_dense() {
    // f(n) = f(n+1) must not get DenseInt.
    let mut f = bare_fun(
        "inc",
        vec![Local(0)],
        Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Binary {
                        op: BinOp::Add,
                        left: Local(0),
                        right: Local(1),
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Call {
                        fun: "inc".into(),
                        args: vec![Local(2)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(3)),
        },
    );
    f.param_names = vec!["n".into()];
    let module = CoreModule {
        name: "M".into(),
        functions: vec![f],
        hash_adts: HashSet::default(),
        trait_methods: HashMap::default(),
    };
    let plan = plan_memo_tf(&module);
    assert!(
        !matches!(plan.get("inc"), Some(MemoTf::DenseInt { .. })),
        "increasing self-recursion must not use dense index T_f, got {:?}",
        plan.get("inc")
    );
}

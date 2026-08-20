use super::*;
use lumia_core::CoreBinOp as BinOp;
use lumia_core::{has_assign_or_name, Block, CoreFun, CoreModule, FunKind, Op, Value};
use lumia_ty::{Effect, Type};

fn pure_add() -> CoreFun {
    // fun add(a, b) { a + b }
    CoreFun {
        name: "add".into(),
        params: vec![Local(0), Local(1)],
        param_names: vec!["a".into(), "b".into()],
        param_tys: vec![Type::Int, Type::Int],
        body: Block {
            ops: vec![Op::Let {
                local: Local(2),
                value: Value::Binary {
                    op: BinOp::Add,
                    left: Local(0),
                    right: Local(1),
                },
                pure_region: true,
            }],
            result: Some(Local(2)),
        },
        ret_ty: Type::Int,
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
    }
}

#[test]
fn inlines_small_pure_call() {
    let mut module = CoreModule::with_functions(
        "M",
        vec![
            pure_add(),
            CoreFun {
                name: "main".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
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
                            value: Value::Call {
                                fun: "add".into(),
                                args: vec![Local(0), Local(1)],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(2)),
                },
                ret_ty: Type::Int,
                effect: Effect::pure(),
                is_main: true,
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
            },
        ],
    );
    inline_module(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").unwrap();
    let has_call = main.body.ops.iter().any(|op| {
        matches!(
            op,
            Op::Let {
                value: Value::Call { fun, .. },
                ..
            } if fun == "add"
        )
    });
    assert!(!has_call, "add call should be inlined");
    let has_add = main.body.ops.iter().any(|op| {
        matches!(
            op,
            Op::Let {
                value: Value::Binary { op: BinOp::Add, .. },
                ..
            }
        )
    });
    assert!(has_add, "inlined body should contain add");
}

#[test]
fn inlines_var_slots_with_renamed_names() {
    // fun bump(n) { var x = n; x = x + 1; x }
    let bump = CoreFun {
        name: "bump".into(),
        params: vec![Local(0)],
        param_names: vec!["n".into()],
        param_tys: vec![Type::Int],
        body: Block {
            ops: vec![
                Op::Assign {
                    name: "x".into(),
                    value: Local(0),
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Name("x".into()),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Binary {
                        op: BinOp::Add,
                        left: Local(1),
                        right: Local(2),
                    },
                    pure_region: true,
                },
                Op::Assign {
                    name: "x".into(),
                    value: Local(3),
                },
                Op::Let {
                    local: Local(4),
                    value: Value::Name("x".into()),
                    pure_region: true,
                },
            ],
            result: Some(Local(4)),
        },
        ret_ty: Type::Int,
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
    };
    assert!(has_assign_or_name(&bump.body));
    assert!(is_inlineable(&bump));

    let mut module = CoreModule::with_functions(
        "M",
        vec![
            bump,
            CoreFun {
                name: "main".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Assign {
                            name: "x".into(),
                            value: Local(0),
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Int(41),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::Call {
                                fun: "bump".into(),
                                args: vec![Local(1)],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(2)),
                },
                ret_ty: Type::Int,
                effect: Effect::pure(),
                is_main: true,
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
            },
        ],
    );
    inline_module(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").unwrap();
    let has_call = main.body.ops.iter().any(|op| {
        matches!(
            op,
            Op::Let {
                value: Value::Call { fun, .. },
                ..
            } if fun == "bump"
        )
    });
    assert!(!has_call, "bump should be inlined");
    // Caller keeps its own `x`; inlined body uses Local-id slot names (`$s…`).
    let assigns: Vec<&str> = main
        .body
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Assign { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(assigns.contains(&"x"));
    assert!(
        assigns.iter().any(|n| n.starts_with("$s")),
        "inlined slots should use $s{{id}} names, got {assigns:?}"
    );
}

#[test]
fn skips_effectful() {
    let mut f = pure_add();
    f.effect = Effect::io();
    assert!(!is_inlineable(&f));
}

#[test]
fn skips_early_return() {
    let f = CoreFun {
        name: "early".into(),
        params: vec![Local(0)],
        param_names: vec!["x".into()],
        param_tys: vec![Type::Int],
        body: Block {
            ops: vec![Op::Return { value: Local(0) }],
            result: None,
        },
        ret_ty: Type::Int,
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
    };
    assert!(!is_inlineable(&f));
}

#[test]
fn mutual_inlineable_pair_does_not_hang() {
    // a calls b, b calls a — both small/pure. Expand stack must cut the cycle.
    let a = CoreFun {
        name: "a".into(),
        params: vec![Local(0)],
        param_names: vec!["x".into()],
        param_tys: vec![Type::Int],
        body: Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Call {
                    fun: "b".into(),
                    args: vec![Local(0)],
                },
                pure_region: true,
            }],
            result: Some(Local(1)),
        },
        ret_ty: Type::Int,
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
    };
    let b = CoreFun {
        name: "b".into(),
        params: vec![Local(0)],
        param_names: vec!["x".into()],
        param_tys: vec![Type::Int],
        body: Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Call {
                    fun: "a".into(),
                    args: vec![Local(0)],
                },
                pure_region: true,
            }],
            result: Some(Local(1)),
        },
        ret_ty: Type::Int,
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
    };
    let mut module = CoreModule::with_functions(
        "M",
        vec![
            a,
            b,
            CoreFun {
                name: "main".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Call {
                                fun: "a".into(),
                                args: vec![Local(0)],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(1)),
                },
                ret_ty: Type::Int,
                effect: Effect::pure(),
                is_main: true,
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
            },
        ],
    );
    inline_module(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").unwrap();
    let ops = count_ops(&main.body);
    assert!(ops < 64, "mutual inline must terminate, got {ops} ops");
}

#[test]
fn inlines_indirect_call_via_funref() {
    // %0 = FunRef(add); %3 = IndirectCall(%0, %1, %2) → expand like Call(add).
    let mut module = CoreModule::with_functions(
        "M",
        vec![
            pure_add(),
            CoreFun {
                name: "main".into(),
                params: vec![],
                param_names: vec![],
                param_tys: vec![],
                body: Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::FunRef("add".into()),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Int(1),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::Int(2),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(3),
                            value: Value::IndirectCall {
                                callee: Local(0),
                                args: vec![Local(1), Local(2)],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(3)),
                },
                ret_ty: Type::Int,
                effect: Effect::pure(),
                is_main: true,
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
            },
        ],
    );
    inline_module(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").unwrap();
    let has_indirect = main.body.ops.iter().any(|op| {
        matches!(
            op,
            Op::Let {
                value: Value::IndirectCall { .. },
                ..
            }
        )
    });
    assert!(!has_indirect, "FunRef IndirectCall should be inlined");
    let has_add = main.body.ops.iter().any(|op| {
        matches!(
            op,
            Op::Let {
                value: Value::Binary { op: BinOp::Add, .. },
                ..
            }
        )
    });
    assert!(has_add, "inlined body should contain add");
}

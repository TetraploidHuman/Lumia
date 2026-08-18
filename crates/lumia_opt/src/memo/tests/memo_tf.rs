use super::*;

#[test]
fn memo_tf_marks_dense_int() {
    // fib-like: f(n) = f(n-1) with enough body weight.
    let mut fib = bare_fun(
        "fib",
        vec![Local(0)],
        Block {
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
    let module = CoreModule::with_functions("M", vec![fib]);
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
        foreign_abi: lumia_core::ForeignAbi::C,
        escaping: HashSet::default(),
        nsw_binop_locals: Default::default(),
        safe_divisor_locals: Default::default(),
        nonneg_iv_load_locals: Default::default(),
        scheme_poly: false,
        mono_of: None,
        kind: FunKind::Normal,
    };
    let module = CoreModule::with_functions("M", vec![sq, main]);
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
    let module = CoreModule::with_functions("M", vec![f]);
    let plan = plan_memo_tf(&module);
    assert!(
        !matches!(plan.get("inc"), Some(MemoTf::DenseInt { .. })),
        "increasing self-recursion must not use dense index T_f, got {:?}",
        plan.get("inc")
    );
}

#[test]
fn memo_tf_user_param_named_env_still_dense() {
    // First param named `env` is a normal binder — must not be treated as a
    // lifted closure (that used to gate on the string `"env"`).
    let mut fib = bare_fun(
        "fib",
        vec![Local(0)],
        Block {
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
    fib.param_names = vec!["env".into()];
    let module = CoreModule::with_functions("M", vec![fib]);
    let plan = plan_memo_tf(&module);
    assert!(
        matches!(plan.get("fib"), Some(MemoTf::DenseInt { .. })),
        "user `env` param must still get DenseInt, got {:?}",
        plan.get("fib")
    );
}

#[test]
fn memo_tf_lifted_lambda_excluded_by_fun_kind() {
    let mut lam = bare_fun(
        "__lam_0",
        vec![Local(0)],
        Block {
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
                        fun: "__lam_0".into(),
                        args: vec![Local(2)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(3)),
        },
    );
    lam.param_names = vec!["n".into()];
    lam.kind = FunKind::LiftedLambda;
    let module = CoreModule::with_functions("M", vec![lam]);
    let plan = plan_memo_tf(&module);
    assert!(
        plan.get("__lam_0").is_none(),
        "lifted lambda must not enter memo plan, got {:?}",
        plan.get("__lam_0")
    );
}

#[test]
fn memo_tf_indirect_self_call_planned_dense() {
    // Same fib-shaped body as DenseInt, but self-recursion is FunRef+IndirectCall.
    // Planner tracks FunRef(self) aliases and counts IndirectCall like Call.
    let mut fib = bare_fun(
        "fib",
        vec![Local(0)],
        Block {
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
                    value: Value::FunRef("fib".into()),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(4),
                    value: Value::IndirectCall {
                        callee: Local(3),
                        args: vec![Local(2)],
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(5),
                    value: Value::Binary {
                        op: BinOp::Add,
                        left: Local(4),
                        right: Local(1),
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(5)),
        },
    );
    fib.param_names = vec!["n".into()];
    let module = CoreModule::with_functions("M", vec![fib]);
    let plan = plan_memo_tf(&module);
    assert!(
        matches!(plan.get("fib"), Some(MemoTf::DenseInt { .. })),
        "FunRef+IndirectCall structural self-rec must get DenseInt, got {:?}",
        plan.get("fib")
    );
}

#[test]
fn memo_tf_indirect_call_other_funref_not_self() {
    // IndirectCall via FunRef("other") must not count as self-recursion.
    let mut fib = bare_fun(
        "fib",
        vec![Local(0)],
        Block {
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
                    value: Value::FunRef("other".into()),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(4),
                    value: Value::IndirectCall {
                        callee: Local(3),
                        args: vec![Local(2)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(4)),
        },
    );
    fib.param_names = vec!["n".into()];
    let module = CoreModule::with_functions("M", vec![fib]);
    let plan = plan_memo_tf(&module);
    assert!(
        plan.get("fib").is_none(),
        "non-self FunRef IndirectCall must not invent DenseInt, got {:?}",
        plan.get("fib")
    );
}

use super::*;

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
    let module = CoreModule::with_functions("M", vec![f]);
    let plan = plan_memo_tf(&module);
    assert!(
        !matches!(plan.get("inc"), Some(MemoTf::DenseInt { .. })),
        "increasing self-recursion must not use dense index T_f, got {:?}",
        plan.get("inc")
    );
}

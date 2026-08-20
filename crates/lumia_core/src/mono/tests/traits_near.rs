use super::{ensure_trait_method_stubs, resolve_trait_method_calls};
use crate::ir::{Block, CoreFun, CoreModule, ForeignAbi, FunKind, Local, Op, Value};
use crate::CoreBinOp as BinOp;
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

fn fun(name: &str, params: Vec<Local>, param_tys: Vec<Type>, body: Block, ret_ty: Type) -> CoreFun {
    CoreFun {
        name: name.into(),
        param_names: params
            .iter()
            .enumerate()
            .map(|(i, _)| format!("p{i}").into())
            .collect(),
        params,
        param_tys,
        body,
        ret_ty,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: None,
        foreign_abi: ForeignAbi::C,
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
fn resolve_trait_binary_add_rewrites_to_single_mangled_impl() {
    let vec2_ty = Type::Adt {
        name: "Vec2".into(),
        params: vec![],
    };
    let mut module = CoreModule::with_functions(
        "M",
        vec![fun(
            "main",
            vec![Local(0), Local(1)],
            vec![vec2_ty.clone(), vec2_ty],
            Block {
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
            Type::Int,
        )],
    );
    module.trait_methods.insert(
        ("Vec2".into(), "add".into()),
        vec!["__Num_Vec2_add".into()],
    );

    resolve_trait_method_calls(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").expect("main");
    assert!(
        matches!(
            &main.body.ops[0],
            Op::Let {
                value: Value::Call { fun, args },
                ..
            } if fun == "__Num_Vec2_add" && args == &vec![Local(0), Local(1)]
        ),
        "expected binary add rewritten to __Num_Vec2_add call, got {:?}",
        main.body.ops[0]
    );
}

#[test]
fn ensure_trait_method_stubs_emits_missing_short_name_with_match_fail() {
    let mut module = CoreModule::with_functions(
        "M",
        vec![
            fun(
                "__Num_Vec2_add_impl",
                vec![Local(0), Local(1)],
                vec![Type::Int, Type::Int],
                Block {
                    ops: vec![],
                    result: Some(Local(0)),
                },
                Type::Int,
            ),
            fun(
                "main",
                vec![Local(0)],
                vec![Type::Int],
                Block {
                    ops: vec![Op::Let {
                        local: Local(1),
                        value: Value::Call {
                            fun: "add".into(),
                            args: vec![Local(0)],
                        },
                        pure_region: true,
                    }],
                    result: Some(Local(1)),
                },
                Type::Int,
            ),
        ],
    );
    module.trait_methods.insert(
        ("Vec2".into(), "add".into()),
        vec!["__Num_Vec2_add_impl".into()],
    );

    ensure_trait_method_stubs(&mut module);
    let add_stub = module
        .functions
        .iter()
        .find(|f| f.name == "add")
        .expect("expected short-name add stub");

    assert!(
        add_stub
            .body
            .ops
            .iter()
            .any(|op| matches!(
                op,
                Op::Let {
                    value: Value::Builtin {
                        name: Builtin::MatchFail,
                        ..
                    },
                    ..
                }
            )),
        "stub must trap via MatchFail, got body={:?}",
        add_stub.body
    );
}

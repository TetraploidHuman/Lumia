use super::rewrite_all_mono_call_sites;
use crate::ir::{Block, CoreFun, CoreModule, ForeignAbi, FunKind, Local, Op, Value};
use crate::mono::key::{MonoKey, MonoKind};
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

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
fn rewrite_direct_call_uses_mono_rename_from_arg_type_key() {
    let mut module = CoreModule::with_functions(
        "M",
        vec![
            fun(
                "foo",
                vec![Local(0)],
                vec![Type::Int],
                Block {
                    ops: vec![],
                    result: Some(Local(0)),
                },
                Type::Int,
            ),
            fun(
                "main",
                vec![],
                vec![],
                Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(7),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::Call {
                                fun: "foo".into(),
                                args: vec![Local(0)],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(1)),
                },
                Type::Int,
            ),
        ],
    );
    let mut renames = HashMap::default();
    renames.insert(
        ("foo".into(), MonoKey(vec![MonoKind::Int])),
        "foo$Int".into(),
    );

    rewrite_all_mono_call_sites(&mut module, &renames);
    let main = module.functions.iter().find(|f| f.name == "main").expect("main");
    assert!(
        matches!(
            &main.body.ops[1],
            Op::Let {
                value: Value::Call { fun, .. },
                ..
            } if fun == "foo$Int"
        ),
        "expected direct call rewritten to foo$Int, got {:?}",
        main.body.ops[1]
    );
}

#[test]
fn rewrite_indirect_call_directizes_to_renamed_clone() {
    let mut module = CoreModule::with_functions(
        "M",
        vec![
            fun(
                "foo",
                vec![Local(0)],
                vec![Type::Int],
                Block {
                    ops: vec![],
                    result: Some(Local(0)),
                },
                Type::Int,
            ),
            fun(
                "main",
                vec![],
                vec![],
                Block {
                    ops: vec![
                        Op::Let {
                            local: Local(0),
                            value: Value::Int(9),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(1),
                            value: Value::FunRef("foo".into()),
                            pure_region: true,
                        },
                        Op::Let {
                            local: Local(2),
                            value: Value::IndirectCall {
                                callee: Local(1),
                                args: vec![Local(0)],
                            },
                            pure_region: true,
                        },
                    ],
                    result: Some(Local(2)),
                },
                Type::Int,
            ),
        ],
    );
    let mut renames = HashMap::default();
    renames.insert(
        ("foo".into(), MonoKey(vec![MonoKind::Int])),
        "foo$Int".into(),
    );

    rewrite_all_mono_call_sites(&mut module, &renames);
    let main = module.functions.iter().find(|f| f.name == "main").expect("main");
    assert!(
        matches!(
            &main.body.ops[2],
            Op::Let {
                value: Value::Call { fun, .. },
                ..
            } if fun == "foo$Int"
        ),
        "expected indirect call directized to foo$Int, got {:?}",
        main.body.ops[2]
    );
}

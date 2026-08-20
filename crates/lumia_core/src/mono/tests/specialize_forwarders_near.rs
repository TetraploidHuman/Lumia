use super::elide_trivial_mono_forwarders;
use crate::ir::{Block, CoreFun, CoreModule, ForeignAbi, FunKind, Local, Op, Value};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

fn fun(
    name: &str,
    params: Vec<Local>,
    param_tys: Vec<Type>,
    body: Block,
    ret_ty: Type,
    mono_of: Option<&str>,
) -> CoreFun {
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
        mono_of: mono_of.map(Into::into),
        kind: FunKind::Normal,
    }
}

#[test]
fn forwarder_chain_collapses_to_final_target() {
    let x = Local(0);
    let a_body = Block {
        ops: vec![Op::Let {
            local: Local(1),
            value: Value::Call {
                fun: "b$Float".into(),
                args: vec![x],
            },
            pure_region: true,
        }],
        result: Some(Local(1)),
    };
    let b_body = Block {
        ops: vec![Op::Let {
            local: Local(1),
            value: Value::Call {
                fun: "c$Float".into(),
                args: vec![x],
            },
            pure_region: true,
        }],
        result: Some(Local(1)),
    };
    let c_body = Block {
        ops: vec![],
        result: Some(x),
    };
    let main_body = Block {
        ops: vec![Op::Let {
            local: Local(1),
            value: Value::Call {
                fun: "a$Float".into(),
                args: vec![Local(2)],
            },
            pure_region: true,
        }],
        result: Some(Local(1)),
    };

    let mut module = CoreModule::empty("M");
    module.functions = vec![
        fun(
            "a$Float",
            vec![x],
            vec![Type::Float],
            a_body,
            Type::Float,
            Some("a"),
        ),
        fun(
            "b$Float",
            vec![x],
            vec![Type::Float],
            b_body,
            Type::Float,
            Some("b"),
        ),
        fun("c$Float", vec![x], vec![Type::Float], c_body, Type::Float, Some("c")),
        fun("main", vec![], vec![], main_body, Type::Float, None),
    ];

    elide_trivial_mono_forwarders(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").expect("main");
    assert!(
        matches!(
            &main.body.ops[0],
            Op::Let {
                value: Value::Call { fun, .. },
                ..
            } if fun == "c$Float"
        ),
        "expected main call rewritten to final target c$Float, got {:?}",
        main.body.ops[0]
    );
}


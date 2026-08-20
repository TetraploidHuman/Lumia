use super::*;
use crate::ir::{Block, CoreFun, FunKind, Local, Op, Value};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

fn fun(body: Block) -> CoreFun {
    CoreFun {
        name: "f".into(),
        params: vec![],
        param_names: vec![],
        param_tys: vec![],
        body,
        ret_ty: Type::Int,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: None,
        foreign_abi: crate::ForeignAbi::C,
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
fn directizes_local_funref_to_call() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::FunRef("g".into()),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::IndirectCall {
                    callee: Local(0),
                    args: vec![Local(2)],
                },
                pure_region: true,
            },
        ],
        result: Some(Local(1)),
    };
    let mut module = CoreModule::empty("m");
    module.functions.push(fun(body));
    directize_funref_calls(&mut module);
    let f = &module.functions[0];
    assert!(
        matches!(
            &f.body.ops[1],
            Op::Let {
                value: Value::Call { fun, args },
                ..
            } if fun == "g" && args == &vec![Local(2)]
        ),
        "expected Call(g), got {:?}",
        f.body.ops[1]
    );
}

#[test]
fn directizes_slot_funref_name_load() {
    // `var next = g; val f = next; f(x)` — Name after Assign must directize.
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::FunRef("g".into()),
                pure_region: true,
            },
            Op::Assign {
                name: "next".into(),
                value: Local(0),
            },
            Op::Let {
                local: Local(1),
                value: Value::Name("next".into()),
                pure_region: true,
            },
            Op::Let {
                local: Local(2),
                value: Value::IndirectCall {
                    callee: Local(1),
                    args: vec![Local(3)],
                },
                pure_region: true,
            },
        ],
        result: Some(Local(2)),
    };
    let mut module = CoreModule::empty("m");
    module.functions.push(fun(body));
    directize_funref_calls(&mut module);
    let f = &module.functions[0];
    assert!(
        matches!(
            &f.body.ops[3],
            Op::Let {
                value: Value::Call { fun, args },
                ..
            } if fun == "g" && args == &vec![Local(3)]
        ),
        "expected Call(g) via slot Name, got {:?}",
        f.body.ops[3]
    );
}

use super::*;
use lumia_core::{Block, CoreFun, FunKind, Local, Op, Value};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

fn fun(body: Block, escaping: HashSet<Local>) -> CoreFun {
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
        foreign_abi: lumia_core::ForeignAbi::C,
        escaping,
        nsw_binop_locals: Default::default(),
        safe_divisor_locals: Default::default(),
        nonneg_iv_load_locals: Default::default(),
        scheme_poly: false,
        mono_of: None,
        kind: FunKind::Normal,
    }
}

#[test]
fn empty_list_is_lit_nonempty_set_is_heap() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::AllocList {
                    elems: vec![],
                    repr: ListRepr::HeapList,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Let {
                local: Local(2),
                value: Value::AllocSet {
                    elems: vec![Local(1)],
                    repr: SetRepr::HeapSet,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(0)),
    };
    let mut module = CoreModule::empty("m");
    module.functions.push(fun(body, HashSet::default()));
    ReprSelect.run(&mut module);
    let ops = &module.functions[0].body.ops;
    match &ops[0] {
        Op::Let {
            value: Value::AllocList { repr, .. },
            ..
        } => assert_eq!(*repr, ListRepr::LitList),
        other => panic!("expected AllocList, got {other:?}"),
    }
    match &ops[2] {
        Op::Let {
            value: Value::AllocSet { repr, .. },
            ..
        } => assert_eq!(*repr, SetRepr::HeapSet),
        other => panic!("expected AllocSet, got {other:?}"),
    }
}

#[test]
fn pe_lit_map_and_set_lower_to_emit_layouts() {
    let body = Block {
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
                value: Value::AllocMap {
                    flat_pairs: vec![Local(0), Local(1)],
                    repr: MapRepr::LitMap,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(3),
                value: Value::AllocSet {
                    elems: vec![Local(0)],
                    repr: SetRepr::LitSet,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(2)),
    };
    let mut module = CoreModule::empty("m");
    module.functions.push(fun(body, HashSet::default()));
    ReprSelect.run(&mut module);
    let ops = &module.functions[0].body.ops;
    match &ops[2] {
        Op::Let {
            value: Value::AllocMap { repr, .. },
            ..
        } => {
            assert!(!repr.is_pe_hint(), "{repr:?}");
            assert_eq!(*repr, MapRepr::HashOrdered);
        }
        other => panic!("expected AllocMap, got {other:?}"),
    }
    match &ops[3] {
        Op::Let {
            value: Value::AllocSet { repr, .. },
            ..
        } => {
            assert!(!repr.is_pe_hint(), "{repr:?}");
            assert_eq!(*repr, SetRepr::HeapSet);
        }
        other => panic!("expected AllocSet, got {other:?}"),
    }
}

#[test]
fn escaping_small_list_stays_heap() {
    let body = Block {
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
                    elems: vec![Local(0), Local(1)],
                    repr: ListRepr::HeapList,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(2)),
    };
    let mut escaping = HashSet::default();
    escaping.insert(Local(2));
    let mut module = CoreModule::empty("m");
    module.functions.push(fun(body, escaping));
    ReprSelect.run(&mut module);
    match &module.functions[0].body.ops[2] {
        Op::Let {
            value: Value::AllocList { repr, .. },
            ..
        } => assert_eq!(*repr, ListRepr::HeapList),
        other => panic!("expected AllocList, got {other:?}"),
    }
}

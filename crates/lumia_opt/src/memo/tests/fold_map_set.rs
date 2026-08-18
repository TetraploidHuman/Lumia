use super::*;

#[test]
fn const_fold_map_get_to_option() {
    use lumia_core::{AdtRepr, MapRepr};
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
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
                            result_ty: None,
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
                            result_ty: None,
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(6)),
            },
        )],
    );
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
            } if lumia_hir::is_option(adt_name) && fields == &[Local(3)]
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
            } if lumia_hir::is_option(adt_name) && fields.is_empty()
        ),
        "map.get(miss) should PE to None, got {:?}",
        module.functions[0].body.ops[8]
    );
}

#[test]
fn const_fold_contains_skips_nonconst_keys() {
    // mapOf(nonconst_key to 2).contains(1) must not fold to false.
    use lumia_core::MapRepr;
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![Local(0)],
            Block {
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
                            result_ty: None,
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(4)),
            },
        )],
    );
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
fn const_fold_map_set_and_set_insert() {
    use lumia_core::{MapRepr, SetRepr};
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
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
                        value: Value::Int(20),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::AllocMap {
                            flat_pairs: vec![Local(0), Local(1)],
                            repr: MapRepr::LitMap,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(4),
                        value: Value::Builtin {
                            name: Builtin::MapSet,
                            args: vec![Local(3), Local(0), Local(2)],
                            result_ty: None,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(5),
                        value: Value::AllocSet {
                            elems: vec![Local(0)],
                            repr: SetRepr::LitSet,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(6),
                        value: Value::Int(2),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(7),
                        value: Value::Builtin {
                            name: Builtin::SetInsert,
                            args: vec![Local(5), Local(6)],
                            result_ty: None,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(8),
                        value: Value::AllocList {
                            elems: vec![Local(0), Local(1), Local(2)],
                            repr: lumia_core::ListRepr::LitList,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(9),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(10),
                        value: Value::Int(99),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(11),
                        value: Value::Builtin {
                            name: Builtin::MapSet,
                            args: vec![Local(8), Local(9), Local(10)],
                            result_ty: None,
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(11)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(
        matches!(
            &module.functions[0].body.ops[4],
            Op::Let {
                value: Value::AllocMap { flat_pairs, .. },
                ..
            } if flat_pairs == &[Local(0), Local(2)]
        ),
        "MapSet should PE upsert, got {:?}",
        module.functions[0].body.ops[4]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[7],
            Op::Let {
                value: Value::AllocSet { elems, .. },
                ..
            } if elems == &[Local(0), Local(6)]
        ),
        "SetInsert should PE, got {:?}",
        module.functions[0].body.ops[7]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[11],
            Op::Let {
                value: Value::AllocList { elems, .. },
                ..
            } if elems == &[Local(0), Local(10), Local(2)]
        ),
        "List.set via MapSet should PE, got {:?}",
        module.functions[0].body.ops[11]
    );
}

#[test]
fn const_fold_compacts_pm0_float_map_set_keys() {
    use lumia_core::{MapRepr, SetRepr};
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Float(0.0),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Unary {
                            op: UnOp::Neg,
                            operand: Local(0),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Int(2),
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
                        value: Value::AllocSet {
                            elems: vec![Local(0), Local(2)],
                            repr: SetRepr::LitSet,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(6),
                        value: Value::Builtin {
                            name: Builtin::ListLen,
                            args: vec![Local(4)],
                            result_ty: None,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(7),
                        value: Value::Builtin {
                            name: Builtin::ListLen,
                            args: vec![Local(5)],
                            result_ty: None,
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(6)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(
        matches!(
            &module.functions[0].body.ops[4],
            Op::Let {
                value: Value::AllocMap { flat_pairs, .. },
                ..
            } if flat_pairs.len() == 2 && flat_pairs[1] == Local(3)
        ),
        "±0 map keys should compact to last value, got {:?}",
        module.functions[0].body.ops[4]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[5],
            Op::Let {
                value: Value::AllocSet { elems, .. },
                ..
            } if elems.len() == 1
        ),
        "±0 set elems should compact, got {:?}",
        module.functions[0].body.ops[5]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[6],
            Op::Let {
                value: Value::Int(1),
                ..
            }
        ),
        "map.len after ±0 compact should be 1, got {:?}",
        module.functions[0].body.ops[6]
    );
    assert!(
        matches!(
            &module.functions[0].body.ops[7],
            Op::Let {
                value: Value::Int(1),
                ..
            }
        ),
        "set.len after ±0 compact should be 1, got {:?}",
        module.functions[0].body.ops[7]
    );
}

use super::*;
use lumia_core::{Block, CoreFun, FunKind, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

fn fun_with_body(body: Block) -> CoreFun {
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
fn result_escapes() {
    let body = Block {
        ops: vec![Op::Let {
            local: Local(0),
            value: Value::Int(1),
            pure_region: true,
        }],
        result: Some(Local(0)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(esc.contains(&Local(0)));
}

#[test]
fn early_return_escapes() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Return { value: Local(0) },
        ],
        result: None,
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(
        esc.contains(&Local(0)),
        "early-return payload must escape (no stack LitAdt)"
    );
}

#[test]
fn dead_temp_does_not_escape() {
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
        ],
        result: Some(Local(1)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(esc.contains(&Local(1)));
    assert!(!esc.contains(&Local(0)));
}

#[test]
fn call_args_escape() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::Call {
                    fun: "g".into(),
                    args: vec![Local(0)],
                },
                pure_region: true,
            },
        ],
        result: Some(Local(1)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(esc.contains(&Local(0)));
    assert!(esc.contains(&Local(1)));
}

#[test]
fn known_pure_len_callee_does_not_escape_arg() {
    use lumia_core::{ListRepr, Op};
    let len_fun = CoreFun {
        name: "len".into(),
        params: vec![Local(0)],
        param_names: vec!["xs".into()],
        param_tys: vec![Type::List(Box::new(Type::Int))],
        body: Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Builtin {
                    name: Builtin::ListLen,
                    args: vec![Local(0)],
                    result_ty: None,
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
    let main_fun = CoreFun {
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
                    value: Value::AllocList {
                        elems: vec![Local(0)],
                        repr: ListRepr::HeapList,
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Call {
                        fun: "len".into(),
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
    };
    let mut module = CoreModule::with_functions("M", vec![len_fun, main_fun]);
    EscapePass.run(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").unwrap();
    assert!(
        !main.escaping.contains(&Local(1)),
        "list passed only to pure len must not escape: {:?}",
        main.escaping
    );
}

#[test]
fn const_specialized_name_summary_hit_does_not_escape() {
    // Escape summaries are keyed by function name strings. After SpecializeConst,
    // clones are named `f$c_N`; Escape must look up that exact key (hit path).
    use lumia_core::{ListRepr, Op};
    let len_fun = CoreFun {
        name: "len$c_0".into(),
        params: vec![Local(0)],
        param_names: vec!["xs".into()],
        param_tys: vec![Type::List(Box::new(Type::Int))],
        body: Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Builtin {
                    name: Builtin::ListLen,
                    args: vec![Local(0)],
                    result_ty: None,
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
    let main_fun = CoreFun {
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
                    value: Value::AllocList {
                        elems: vec![Local(0)],
                        repr: ListRepr::HeapList,
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Call {
                        fun: "len$c_0".into(),
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
    };
    let mut module = CoreModule::with_functions("M", vec![len_fun, main_fun]);
    EscapePass.run(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").unwrap();
    assert!(
        !main.escaping.contains(&Local(1)),
        "Call to matching `$c_` summary key must hit (not miss→escape-all): {:?}",
        main.escaping
    );
}

#[test]
fn const_specialized_name_summary_miss_over_escapes() {
    // Divergent keys: body cloned as `len$c_0` but Call still says `len` → miss
    // marks all args escaping (safe over-approx). Lock-in of the string-key hazard.
    use lumia_core::{ListRepr, Op};
    let len_fun = CoreFun {
        name: "len$c_0".into(),
        params: vec![Local(0)],
        param_names: vec!["xs".into()],
        param_tys: vec![Type::List(Box::new(Type::Int))],
        body: Block {
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Builtin {
                    name: Builtin::ListLen,
                    args: vec![Local(0)],
                    result_ty: None,
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
    let main_fun = CoreFun {
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
                    value: Value::AllocList {
                        elems: vec![Local(0)],
                        repr: ListRepr::HeapList,
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Call {
                        fun: "len".into(),
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
    };
    let mut module = CoreModule::with_functions("M", vec![len_fun, main_fun]);
    EscapePass.run(&mut module);
    let main = module.functions.iter().find(|f| f.name == "main").unwrap();
    assert!(
        main.escaping.contains(&Local(1)),
        "Call/summary name mismatch must miss→escape-all: {:?}",
        main.escaping
    );
}

#[test]
fn alloc_elems_escape_when_list_returned() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::Int(7),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::AllocList {
                    elems: vec![Local(0)],
                    repr: lumia_core::ListRepr::HeapList,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(1)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(esc.contains(&Local(1)));
    assert!(esc.contains(&Local(0)));
}

#[test]
fn short_lived_var_assign_does_not_escape() {
    // `var xs = listOf(1)` used only for a pure projection must not escape.
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::AllocList {
                    elems: vec![Local(0)],
                    repr: lumia_core::ListRepr::HeapList,
                },
                pure_region: true,
            },
            Op::Assign {
                name: "xs".into(),
                value: Local(1),
            },
            Op::Let {
                local: Local(2),
                value: Value::Builtin {
                    name: lumia_hir::Builtin::ListLen,
                    args: vec![Local(1)],
                    result_ty: None,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(2)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(
        !esc.contains(&Local(1)),
        "non-escaping var list must stay stack-eligible: {esc:?}"
    );
}

#[test]
fn var_name_read_that_returns_escapes_assigns() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::AllocList {
                    elems: vec![],
                    repr: lumia_core::ListRepr::HeapList,
                },
                pure_region: true,
            },
            Op::Assign {
                name: "xs".into(),
                value: Local(0),
            },
            Op::Let {
                local: Local(1),
                value: Value::Name("xs".into()),
                pure_region: true,
            },
        ],
        result: Some(Local(1)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(
        esc.contains(&Local(0)),
        "returning Name(xs) must escape assigns to xs: {esc:?}"
    );
}

#[test]
fn returned_take_escapes_source_list() {
    // Take copies element pointers — source must escape when take result does.
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::AllocList {
                    elems: vec![Local(0)],
                    repr: lumia_core::ListRepr::HeapList,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(2),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Let {
                local: Local(3),
                value: Value::Builtin {
                    name: Builtin::ListTake,
                    args: vec![Local(1), Local(2)],
                    result_ty: None,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(3)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(
        esc.contains(&Local(1)),
        "source list of escaping Take must escape: {esc:?}"
    );
    assert!(esc.contains(&Local(0)), "list elems must escape: {esc:?}");
}

#[test]
fn dead_take_does_not_force_source_escape() {
    // Take/Slice are not may_capture: a non-escaping take result must not force
    // the source list onto the heap (GC mark_value no-ops non-heap elem bits).
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::AllocList {
                    elems: vec![Local(0)],
                    repr: lumia_core::ListRepr::HeapList,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(2),
                value: Value::Int(1),
                pure_region: true,
            },
            Op::Let {
                local: Local(3),
                value: Value::Builtin {
                    name: Builtin::ListTake,
                    args: vec![Local(1), Local(2)],
                    result_ty: None,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(4),
                value: Value::Builtin {
                    name: Builtin::ListLen,
                    args: vec![Local(3)],
                    result_ty: None,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(4)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(
        !esc.contains(&Local(1)),
        "dead take must not escape source list: {esc:?}"
    );
    assert!(
        !esc.contains(&Local(3)),
        "take result used only for len must not escape: {esc:?}"
    );
}

#[test]
fn returned_list_get_escapes_source_list() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::AllocAdt {
                    adt_name: "Point".into(),
                    tag: 0,
                    fields: vec![],
                    repr: lumia_core::AdtRepr::HeapAdt,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::AllocList {
                    elems: vec![Local(0)],
                    repr: lumia_core::ListRepr::HeapList,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(2),
                value: Value::Int(0),
                pure_region: true,
            },
            Op::Let {
                local: Local(3),
                value: Value::Builtin {
                    name: Builtin::ListGet,
                    args: vec![Local(1), Local(2)],
                    result_ty: None,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(3)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(
        esc.contains(&Local(1)),
        "list of escaping ListGet must escape: {esc:?}"
    );
    assert!(
        esc.contains(&Local(0)),
        "elem behind escaping ListGet must escape: {esc:?}"
    );
}

#[test]
fn wide_heap_adt_fields_escape_even_if_adt_local_does_not() {
    // ReprSelect: >8 fields ⇒ HeapAdt even when the product itself does not escape.
    // Stack LitList fields stored in that heap object are GC-invisible / UAF.
    let mut fields = Vec::new();
    let mut ops = Vec::new();
    for i in 0..9 {
        ops.push(Op::Let {
            local: Local(i),
            value: Value::Int(i as i64),
            pure_region: true,
        });
        fields.push(Local(i));
    }
    // Small list used only as a wide-product field (product result unused).
    ops.push(Op::Let {
        local: Local(9),
        value: Value::AllocList {
            elems: vec![Local(0)],
            repr: lumia_core::ListRepr::HeapList,
        },
        pure_region: true,
    });
    fields.push(Local(9));
    ops.push(Op::Let {
        local: Local(10),
        value: Value::AllocAdt {
            adt_name: "Wide".into(),
            tag: 0,
            fields,
            repr: lumia_core::AdtRepr::HeapAdt,
        },
        pure_region: true,
    });
    ops.push(Op::Let {
        local: Local(11),
        value: Value::Int(0),
        pure_region: true,
    });
    let body = Block {
        ops,
        result: Some(Local(11)),
    };
    let esc = escaping_locals(&fun_with_body(body));
    assert!(
        !esc.contains(&Local(10)),
        "wide product itself need not escape: {esc:?}"
    );
    assert!(
        esc.contains(&Local(9)),
        "list field of non-escaping HeapAdt must escape (no stack LitList): {esc:?}"
    );
}

#[test]
fn mutual_recursion_scc_params_need_not_escape() {
    // even(n) / odd(n) call each other with Int — params must not escape.
    // Also locks worklist fixed-point (not whole-module force on converge).
    fn leaf(name: &str, peer: &str) -> CoreFun {
        CoreFun {
            name: name.into(),
            params: vec![Local(0)],
            param_names: vec!["n".into()],
            param_tys: vec![Type::Int],
            body: Block {
                ops: vec![
                    Op::Let {
                        local: Local(1),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Binary {
                            op: lumia_core::CoreBinOp::Sub,
                            left: Local(0),
                            right: Local(1),
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(3),
                        value: Value::Call {
                            fun: peer.into(),
                            args: vec![Local(2)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(3)),
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
    let mut module =
        CoreModule::with_functions("M", vec![leaf("even", "odd"), leaf("odd", "even")]);
    let summaries = super::compute_param_escape_summaries(&module);
    let even_id = *summaries.name_to_id.get("even").unwrap();
    let odd_id = *summaries.name_to_id.get("odd").unwrap();
    assert_eq!(
        summaries.by_id.get(&even_id).map(|v| v.as_slice()),
        Some([false].as_slice()),
        "even param must not escape"
    );
    assert_eq!(
        summaries.by_id.get(&odd_id).map(|v| v.as_slice()),
        Some([false].as_slice()),
        "odd param must not escape"
    );

    let graph = super::call_graph::EscapeCallGraph::from_module(&module, &summaries);
    let idx = super::call_graph::scc_index_map(&graph);
    assert_eq!(
        idx.get(&even_id),
        idx.get(&odd_id),
        "even/odd must share an SCC"
    );

    EscapePass.run(&mut module);
    for f in &module.functions {
        assert!(
            !f.escaping.contains(&Local(0)),
            "{} param Local(0) must not escape: {:?}",
            f.name,
            f.escaping
        );
    }
}

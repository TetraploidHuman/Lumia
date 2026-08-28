use super::*;
use crate::Pass;
use lumi_core::{Block, CoreFun, Local, Op, Value};
use lumi_hir::Builtin;
use lumi_ty::{Effect, Type};
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
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
    }
}

#[test]
fn result_escapes() {
    let body = Block {
        params: vec![],
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
        params: vec![],
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
        params: vec![],
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
        params: vec![],
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
    use lumi_core::{ListRepr, Op};
    let len_fun = CoreFun {
        name: "len".into(),
        params: vec![Local(0)],
        param_names: vec!["xs".into()],
        param_tys: vec![Type::List(Box::new(Type::Int))],
        body: Block {
            params: vec![],
            ops: vec![Op::Let {
                local: Local(1),
                value: Value::Builtin {
                    name: Builtin::ListLen,
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
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
    };
    let main_fun = CoreFun {
        name: "main".into(),
        params: vec![],
        param_names: vec![],
        param_tys: vec![],
        body: Block {
            params: vec![],
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
        escaping: HashSet::default(),
        scheme_poly: false,
        mono_of: None,
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
fn alloc_elems_escape_when_list_returned() {
    let body = Block {
        params: vec![],
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
                    repr: lumi_core::ListRepr::HeapList,
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
        params: vec![],
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
                    repr: lumi_core::ListRepr::HeapList,
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
                    name: lumi_hir::Builtin::ListLen,
                    args: vec![Local(1)],
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
        params: vec![],
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::AllocList {
                    elems: vec![],
                    repr: lumi_core::ListRepr::HeapList,
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
        params: vec![],
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
                    repr: lumi_core::ListRepr::HeapList,
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
    // the source list onto the heap. Escaping take still marks the source
    // (Slice retains the parent buffer).
    let body = Block {
        params: vec![],
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
                    repr: lumi_core::ListRepr::HeapList,
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
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(4),
                value: Value::Builtin {
                    name: Builtin::ListLen,
                    args: vec![Local(3)],
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
        params: vec![],
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::AllocAdt {
                    adt_name: "Point".into(),
                    tag: 0,
                    fields: vec![],
                    repr: lumi_core::AdtRepr::HeapAdt,
                },
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::AllocList {
                    elems: vec![Local(0)],
                    repr: lumi_core::ListRepr::HeapList,
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
            repr: lumi_core::ListRepr::HeapList,
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
            repr: lumi_core::AdtRepr::HeapAdt,
        },
        pure_region: true,
    });
    ops.push(Op::Let {
        local: Local(11),
        value: Value::Int(0),
        pure_region: true,
    });
    let body = Block {
        params: vec![],
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

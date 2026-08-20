use super::{
    constant_returned_adt_funrefs_in_body, constant_returned_funref_in_body,
    constant_returned_list_funrefs_in_body, FunrefElem,
};
use crate::ir::{AdtRepr, Block, ListRepr, Local, Op, Value};

#[test]
fn constant_returned_funref_chases_local_alias_chain() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::FunRef("g".into()),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::Local(Local(0)),
                pure_region: true,
            },
        ],
        result: Some(Local(1)),
    };
    assert_eq!(
        constant_returned_funref_in_body(&body).as_deref(),
        Some("g")
    );
}

#[test]
fn constant_returned_list_funrefs_tracks_nested_funrefs() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::FunRef("f".into()),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::Local(Local(0)),
                pure_region: true,
            },
            Op::Let {
                local: Local(2),
                value: Value::AllocList {
                    elems: vec![Local(1)],
                    repr: ListRepr::HeapList,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(2)),
    };
    let slots = constant_returned_list_funrefs_in_body(&body).expect("list funrefs");
    assert_eq!(slots.len(), 1);
    assert!(matches!(slots[0], Some(FunrefElem::Fun(ref n)) if n == "f"));
}

#[test]
fn constant_returned_adt_funrefs_tracks_nested_funrefs() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::FunRef("h".into()),
                pure_region: true,
            },
            Op::Let {
                local: Local(1),
                value: Value::AllocAdt {
                    adt_name: "Box".into(),
                    tag: 0,
                    fields: vec![Local(0)],
                    repr: AdtRepr::HeapAdt,
                },
                pure_region: true,
            },
        ],
        result: Some(Local(1)),
    };
    let slots = constant_returned_adt_funrefs_in_body(&body).expect("adt funrefs");
    assert_eq!(slots.len(), 1);
    assert!(matches!(slots[0], Some(FunrefElem::Fun(ref n)) if n == "h"));
}


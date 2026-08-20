// Extracted from captures.rs (Todo: 测外置).

use super::*;
use crate::ir::{Block, Local, Op, Value};

fn empty_block() -> Block {
    Block {
        ops: vec![],
        result: None,
    }
}

#[test]
fn local_mut_loop_counter_is_not_captured() {
    // `__i := 1; loop { load __i; ... }` — classic `for` lowering.
    let body = Block {
        ops: vec![
            Op::Assign {
                name: "__i".into(),
                value: Local(0),
            },
            Op::Let {
                local: Local(1),
                value: Value::Loop {
                    header: Box::new(Block {
                        ops: vec![Op::Let {
                            local: Local(2),
                            value: Value::Name("__i".into()),
                            pure_region: true,
                        }],
                        result: Some(Local(2)),
                    }),
                    body: Box::new(empty_block()),
                    latch: Box::new(Block {
                        ops: vec![Op::Assign {
                            name: "__i".into(),
                            value: Local(3),
                        }],
                        result: None,
                    }),
                },
                pure_region: true,
            },
        ],
        result: None,
    };
    let (_, free_names) = analyze_captures(&body, &[]);
    assert!(free_names.is_empty(), "{free_names:?}");
}

#[test]
fn outer_mut_load_is_captured() {
    let body = Block {
        ops: vec![
            Op::Let {
                local: Local(0),
                value: Value::Name("n".into()),
                pure_region: true,
            },
            Op::Assign {
                name: "n".into(),
                value: Local(0),
            },
        ],
        result: None,
    };
    let (_, free_names) = analyze_captures(&body, &[Local(10)]);
    assert_eq!(free_names, vec![lumia_syntax::Sym::from("n")]);
}

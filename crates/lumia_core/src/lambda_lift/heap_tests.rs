use super::*;
use crate::ir::{Block, Local, Op, Value};
use lumia_hir::Builtin;

#[test]
fn result_heap_never_builtins_are_non_heap() {
    assert!(!builtin_result_may_heap(Builtin::ListLen, None));
    assert!(!builtin_result_may_heap(Builtin::ChannelSend, None));
    assert!(!builtin_result_may_heap(Builtin::ScopeEnter, None));
}

#[test]
fn typed_recv_join_unstamped_stay_non_heap() {
    assert!(!builtin_result_may_heap(Builtin::ChannelRecv, None));
    assert!(!builtin_result_may_heap(Builtin::TaskJoin, None));
    // Other Typed projections stay conservative without a stamp.
    assert!(builtin_result_may_heap(Builtin::ListGet, None));
    assert!(builtin_result_may_heap(Builtin::AdtField, None));
}

#[test]
fn typed_recv_join_stamped_follow_type_may_heap() {
    assert!(builtin_result_may_heap(
        Builtin::ChannelRecv,
        Some(&Type::List(Box::new(Type::Float)))
    ));
    assert!(builtin_result_may_heap(
        Builtin::TaskJoin,
        Some(&Type::String)
    ));
    assert!(!builtin_result_may_heap(
        Builtin::ChannelRecv,
        Some(&Type::Int)
    ));
    assert!(!builtin_result_may_heap(
        Builtin::TaskJoin,
        Some(&Type::Float)
    ));
    assert!(builtin_result_may_heap(
        Builtin::ChannelRecv,
        Some(&Type::Char)
    ));
}

#[test]
fn stamped_list_recv_block_is_may_heap() {
    let body = Block {
        ops: vec![Op::Let {
            local: Local(1),
            value: Value::Builtin {
                name: Builtin::ChannelRecv,
                args: vec![Local(0)],
                result_ty: Some(Type::List(Box::new(Type::Int))),
            },
            pure_region: false,
        }],
        result: Some(Local(1)),
    };
    assert!(block_result_may_heap_with_params(&body, &[Local(0)]));
}

#[test]
fn identity_param_block_is_may_heap() {
    // `{ s -> s }` — param is treated as maybe-heap for call-site rooting.
    let body = Block {
        ops: vec![],
        result: Some(Local(0)),
    };
    assert!(block_result_may_heap_with_params(&body, &[Local(0)]));
}

#[test]
fn list_len_result_is_non_heap() {
    let body = Block {
        ops: vec![Op::Let {
            local: Local(1),
            value: Value::Builtin {
                name: Builtin::ListLen,
                args: vec![Local(0)],
                result_ty: None,
            },
            pure_region: false,
        }],
        result: Some(Local(1)),
    };
    assert!(!block_result_may_heap_with_params(&body, &[Local(0)]));
}

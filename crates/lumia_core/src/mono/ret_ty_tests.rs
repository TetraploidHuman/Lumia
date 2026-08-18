use super::{merge_slot_ty, param_ty_map, refine_mono_container_ret};
use crate::ir::{Block, CoreFun, FunKind, Local};
use crate::ForeignAbi;
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

#[test]
fn param_ty_map_zips_params() {
    let fun = CoreFun {
        name: "f".into(),
        params: vec![Local(0), Local(1)],
        param_names: vec!["a".into(), "b".into()],
        param_tys: vec![Type::Int, Type::Float],
        body: Block {
            ops: vec![],
            result: None,
        },
        ret_ty: Type::Unit,
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
    };
    let m = param_ty_map(&fun);
    assert_eq!(m.get(&0), Some(&Type::Int));
    assert_eq!(m.get(&1), Some(&Type::Float));
}

#[test]
fn refine_mono_container_fills_var_slots_only() {
    let orig = Type::Adt {
        name: "Option".into(),
        params: vec![Type::Var(0)],
    };
    let inferred = Type::Adt {
        name: "Option".into(),
        params: vec![Type::Float],
    };
    assert_eq!(
        refine_mono_container_ret(&orig, &inferred),
        Type::Adt {
            name: "Option".into(),
            params: vec![Type::Float],
        }
    );
    // Concrete Int placeholder must not be blasted by Float inference.
    let orig_int = Type::Adt {
        name: "Option".into(),
        params: vec![Type::Int],
    };
    assert_eq!(refine_mono_container_ret(&orig_int, &inferred), orig_int);
    assert_eq!(
        refine_mono_container_ret(
            &Type::List(Box::new(Type::Var(1))),
            &Type::List(Box::new(Type::Float))
        ),
        Type::List(Box::new(Type::Float))
    );
}

#[test]
fn merge_slot_ty_heap_beats_float() {
    // Char/String are heap pointers — must not lose to Float (old is_ref_ty
    // omitted Char and could store a Char pointer in an XMM slot).
    assert_eq!(merge_slot_ty(Some(Type::Char), Type::Float), Type::Char);
    assert_eq!(merge_slot_ty(Some(Type::Float), Type::Char), Type::Char);
    assert_eq!(merge_slot_ty(Some(Type::String), Type::Float), Type::String);
    assert_eq!(
        merge_slot_ty(Some(Type::List(Box::new(Type::Int))), Type::Int),
        Type::List(Box::new(Type::Int))
    );
    // Int-only tuples are not heap pointers under `type_may_heap`.
    assert_eq!(
        merge_slot_ty(Some(Type::Tuple(vec![Type::Int, Type::Int])), Type::Float),
        Type::Float
    );
}

#[test]
fn join_fixed_float_beats_string_and_bool() {
    use crate::join_fixed_ty;
    assert_eq!(
        join_fixed_ty(&Type::String, &Type::Float),
        Some(Type::Float)
    );
    assert_eq!(join_fixed_ty(&Type::Float, &Type::Bool), Some(Type::Float));
    assert_eq!(join_fixed_ty(&Type::Char, &Type::Float), Some(Type::Float));
}

#[test]
fn join_fixed_keeps_fun_over_string() {
    use crate::join_fixed_ty;
    let f = Type::Fun(vec![], Box::new(Type::Int), Effect::pure());
    assert_eq!(join_fixed_ty(&f, &Type::String), Some(f.clone()));
    assert_eq!(join_fixed_ty(&Type::String, &f), Some(f));
}

#[test]
fn join_fixed_merges_list_and_result_float() {
    use crate::join_fixed_ty;
    assert_eq!(
        join_fixed_ty(
            &Type::List(Box::new(Type::Int)),
            &Type::List(Box::new(Type::Float))
        ),
        Some(Type::List(Box::new(Type::Float)))
    );
    let a = Type::Adt {
        name: "Result".into(),
        params: vec![Type::String, Type::Int],
    };
    let b = Type::Adt {
        name: "Result".into(),
        params: vec![Type::Float, Type::Int],
    };
    assert_eq!(
        join_fixed_ty(&a, &b),
        Some(Type::Adt {
            name: "Result".into(),
            params: vec![Type::Float, Type::Int],
        })
    );
}

#[test]
fn join_fixed_fun_fun_merges_rets() {
    use crate::join_fixed_ty;
    let a = Type::Fun(vec![Type::Int], Box::new(Type::Int), Effect::pure());
    let b = Type::Fun(vec![Type::Int], Box::new(Type::Float), Effect::pure());
    assert_eq!(
        join_fixed_ty(&a, &b),
        Some(Type::Fun(
            vec![Type::Int],
            Box::new(Type::Float),
            Effect::pure()
        ))
    );
}

#[test]
fn via_task_join_rejects_channel_recv() {
    // Gate split: TaskJoin must not accept Channel recv (parity with float_abi).
    use crate::value_ty::via_gated_recv;
    use lumia_hir::Builtin;
    let args = [Local(0)];
    let recv = Type::Channel(Box::new(Type::Int));
    assert!(via_gated_recv(Builtin::TaskJoin, &args, recv.clone(), |t| {
        matches!(t, Type::Task(_))
    })
    .is_none());
    assert!(via_gated_recv(Builtin::ChannelRecv, &args, recv, |t| {
        matches!(t, Type::Channel(_))
    })
    .is_some());
}

use super::super::key::{MonoKey, MonoKind};
use crate::ir::{Block, CoreFun, ForeignAbi, FunKind, Local};
use lumia_ty::{Effect, Type};

#[test]
fn args_mono_key_accepts_tuple_float() {
    use super::super::key::args_mono_key;
    use lumia_ty::Type;
    use rustc_hash::FxHashMap as HashMap;
    use crate::ir::Local;
    let mut local_tys = HashMap::default();
    local_tys.insert(0, Type::Tuple(vec![Type::Float, Type::Int]));
    let key = args_mono_key(&[Local(0)], &local_tys, &HashMap::default(), None)
        .expect("Tuple[Float,Int] must be keyable");
    assert_eq!(key.suffix(), "$Tuple_Float_Int");
}

#[test]
fn args_mono_key_accepts_unit() {
    use super::super::key::args_mono_key;
    use lumia_ty::Type;
    use rustc_hash::FxHashMap as HashMap;
    use crate::ir::Local;
    let mut local_tys = HashMap::default();
    local_tys.insert(0, Type::Unit);
    local_tys.insert(1, Type::Float);
    let key = args_mono_key(&[Local(0), Local(1)], &local_tys, &HashMap::default(), None)
        .expect("Unit+Float must be keyable");
    assert_eq!(key.suffix(), "$Unit_Float");
}

#[test]
fn args_mono_key_rejects_fun_with_open_param() {
    use super::super::key::args_mono_key;
    use rustc_hash::FxHashMap as HashMap;
    let mut local_tys = HashMap::default();
    // Open Fun param must not collapse to `Fun_Int_Float` via unwrap_or(Int).
    local_tys.insert(
        0,
        Type::Fun(
            vec![Type::Var(0)],
            Box::new(Type::Float),
            Effect::pure(),
        ),
    );
    assert!(
        args_mono_key(&[Local(0)], &local_tys, &HashMap::default(), None).is_none(),
        "unkeyable Fun child must fail the whole mono key"
    );
}

#[test]
fn args_mono_key_accepts_ground_fun() {
    use super::super::key::args_mono_key;
    use rustc_hash::FxHashMap as HashMap;
    let mut local_tys = HashMap::default();
    local_tys.insert(
        0,
        Type::Fun(
            vec![Type::Float],
            Box::new(Type::Float),
            Effect::pure(),
        ),
    );
    let key = args_mono_key(&[Local(0)], &local_tys, &HashMap::default(), None)
        .expect("ground Fun");
    assert_eq!(key.suffix(), "$Fun_Float_Float");
}

#[test]
fn funref_to_type_is_not_fake_zero_ary_fun() {
    assert!(
        !matches!(
            MonoKind::FunRef("dbl".into()).to_type(),
            Type::Fun(ps, _, _) if ps.is_empty()
        ),
        "FunRef::to_type must not look like Fun([], …)"
    );
    let funs = [CoreFun {
        name: "dbl".into(),
        params: vec![Local(0)],
        param_names: vec!["x".into()],
        param_tys: vec![Type::Float],
        ret_ty: Type::Float,
        effect: Effect::pure(),
        body: Block {
            ops: vec![],
            result: None,
        },
        is_main: false,
        external: None,
        foreign_abi: ForeignAbi::C,
        memo: None,
        escaping: Default::default(),
        scheme_poly: false,
        mono_of: None,
        kind: FunKind::Normal,
    }];
    let key = MonoKey(vec![MonoKind::FunRef("dbl".into())]);
    let tys = key.param_tys(&funs);
    assert!(
        matches!(&tys[0], Type::Fun(ps, r, _) if ps.len() == 1 && matches!(r.as_ref(), Type::Float)),
        "param_tys must resolve FunRef via CoreFun"
    );
    assert!(
        matches!(
            key.ret_ty(&funs, None),
            Type::Fun(ps, r, _) if ps.len() == 1 && matches!(*r, Type::Float)
        ),
        "homogeneous FunRef key ret_ty must resolve via CoreFun"
    );
}

#[test]
fn args_mono_key_prefers_formal_adt_over_abi_int() {
    use super::super::key::args_mono_key;
    use rustc_hash::FxHashMap as HashMap;
    let mut local_tys = HashMap::default();
    local_tys.insert(0, Type::Int); // ABI-erased product
    local_tys.insert(1, Type::Float);
    let formals = [
        Type::Adt {
            name: "Parts".into(),
            params: vec![Type::Int, Type::Float],
        },
        Type::Float,
    ];
    let key = args_mono_key(
        &[Local(0), Local(1)],
        &local_tys,
        &HashMap::default(),
        Some(&formals),
    )
    .expect("key");
    assert_eq!(key.suffix(), "$Parts_Float");
    let tys = super::super::key::materialize_mono_param_tys(&key, &formals, &[]);
    assert!(matches!(
        &tys[0],
        Type::Adt { name, params } if name == "Parts" && params.len() == 2
    ));
    assert!(matches!(tys[1], Type::Float));
}

#[test]
fn mono_key_ret_ty_prefers_list_over_trailing_float() {
    let key = MonoKey(vec![
        MonoKind::List(Box::new(MonoKind::Float)),
        MonoKind::Float,
    ]);
    assert_eq!(key.ret_ty(&[], None), Type::List(Box::new(Type::Float)));
}

#[test]
fn mono_key_suffix_homogeneous_scalars() {
    assert_eq!(
        MonoKey(vec![MonoKind::Float, MonoKind::Float]).suffix(),
        "$Float"
    );
    assert_eq!(MonoKey(vec![MonoKind::Bool]).suffix(), "$Bool");
    assert_eq!(MonoKey(vec![MonoKind::String]).suffix(), "$String");
    assert_eq!(
        MonoKey(vec![MonoKind::List(Box::new(MonoKind::Int))]).suffix(),
        "$List_Int"
    );
    assert_eq!(
        MonoKey(vec![
            MonoKind::Adt {
                name: lumia_hir::OPTION.name.into(),
                params: vec![MonoKind::Float],
            },
            MonoKind::FunRef("dbl".into()),
        ])
        .suffix(),
        "$Option_Float_Fn_dbl"
    );
}

#[test]
fn mono_key_hof_ret_ty_option_map() {
    let dbl = CoreFun {
        name: "dbl".into(),
        params: vec![Local(0)],
        param_names: vec!["x".into()],
        param_tys: vec![Type::Float],
        ret_ty: Type::Float,
        effect: Effect::pure(),
        body: Block {
            ops: vec![],
            result: None,
        },
        is_main: false,
        external: None,
        foreign_abi: ForeignAbi::C,
        memo: None,
        escaping: Default::default(),
        scheme_poly: false,
        mono_of: None,
        kind: FunKind::Normal,
    };
    let key = MonoKey(vec![
        MonoKind::Adt {
            name: lumia_hir::OPTION.name.into(),
            params: vec![MonoKind::Int],
        },
        MonoKind::FunRef("dbl".into()),
    ]);
    let ret = key
        .hof_ret_ty(std::slice::from_ref(&dbl), Some("optionMap"))
        .expect("hof ret");
    assert_eq!(
        ret,
        Type::Adt {
            name: lumia_hir::OPTION.name.into(),
            params: vec![Type::Float],
        }
    );
}

use super::upgrade_generic_param_tys_from_clones;
use crate::ir::{Block, CoreFun, CoreModule, ForeignAbi, FunKind, Local};
use lumia_ty::{Effect, Type};
use rustc_hash::FxHashSet as HashSet;

fn fun(
    name: &str,
    param_ty: Type,
    ret_ty: Type,
    mono_of: Option<&str>,
    scheme_poly: bool,
) -> CoreFun {
    CoreFun {
        name: name.into(),
        params: vec![Local(0)],
        param_names: vec!["x".into()],
        param_tys: vec![param_ty],
        body: Block {
            ops: vec![],
            result: Some(Local(0)),
        },
        ret_ty,
        effect: Effect::pure(),
        is_main: false,
        memo: None,
        external: None,
        foreign_abi: ForeignAbi::C,
        escaping: HashSet::default(),
        nsw_binop_locals: Default::default(),
        safe_divisor_locals: Default::default(),
        nonneg_iv_load_locals: Default::default(),
        scheme_poly,
        mono_of: mono_of.map(Into::into),
        kind: FunKind::Normal,
    }
}

#[test]
fn upgrades_non_scheme_poly_generic_from_more_precise_clone() {
    let mut module = CoreModule::empty("M");
    module.functions = vec![
        fun("dbl", Type::Int, Type::Int, None, false),
        fun("dbl$Float", Type::Float, Type::Float, Some("dbl"), false),
    ];

    upgrade_generic_param_tys_from_clones(&mut module);
    let dbl = module.functions.iter().find(|f| f.name == "dbl").expect("dbl");
    assert_eq!(dbl.param_tys, vec![Type::Float]);
    assert_eq!(dbl.ret_ty, Type::Float);
}

#[test]
fn does_not_upgrade_scheme_poly_generic_from_clone() {
    let mut module = CoreModule::empty("M");
    module.functions = vec![
        fun("id", Type::Int, Type::Int, None, true),
        fun("id$Float", Type::Float, Type::Float, Some("id"), false),
    ];

    upgrade_generic_param_tys_from_clones(&mut module);
    let id = module.functions.iter().find(|f| f.name == "id").expect("id");
    assert_eq!(id.param_tys, vec![Type::Int]);
    assert_eq!(id.ret_ty, Type::Int);
}


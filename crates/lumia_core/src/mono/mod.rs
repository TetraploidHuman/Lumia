//! Monomorphization, FunRef directization, and trait-method resolve.

mod fun_index;
mod key;
mod ret_ty;
mod specialize;
mod traits;

pub(crate) use specialize::specialize_mono_calls;
pub(crate) use traits::{
    directize_funref_calls, ensure_trait_method_stubs, resolve_trait_method_calls,
};

#[cfg(test)]
mod tests {
    use super::key::{MonoKey, MonoKind};
    use crate::compile_source_to_core;
    use crate::ir::{Block, CoreFun, Local};
    use lumia_ty::{Effect, Type};
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
                    name: "Option".into(),
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
                params: vec![],
                ops: vec![],
                result: None,
            },
            is_main: false,
            external: None,
            memo: None,
            escaping: Default::default(),
            scheme_poly: false,
        };
        let key = MonoKey(vec![
            MonoKind::Adt {
                name: "Option".into(),
                params: vec![MonoKind::Int],
            },
            MonoKind::FunRef("dbl".into()),
        ]);
        let ret = key.hof_ret_ty(std::slice::from_ref(&dbl)).expect("hof ret");
        assert_eq!(
            ret,
            Type::Adt {
                name: "Option".into(),
                params: vec![Type::Float],
            }
        );
    }

    #[test]
    fn specialize_clones_poly_dbl_for_float() {
        let core = compile_source_to_core(
            r#"
module M
val dbl = { x -> x + x }
val main = {
    dbl(1)
    dbl(1.5)
}
"#,
        )
        .expect("core");
        assert!(
            core.functions
                .iter()
                .any(|f| f.name.contains("dbl") && f.name.contains("$Float")),
            "expected dbl$Float clone, funs={:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn refresh_upgrades_hof_float_apply_ret() {
        let core = compile_source_to_core(
            r#"
module M
val dbl = { x -> x + x }
val apply = { f, x -> f(x) }
val main = {
    apply(dbl, 1.5)
}
"#,
        )
        .expect("core");
        let apply_clone = core
            .functions
            .iter()
            .find(|f| f.name.contains("apply") && f.name.contains('$'))
            .expect("apply mono clone");
        assert!(
            matches!(apply_clone.ret_ty, Type::Float),
            "apply clone ret_ty should be Float after refresh, got {:?}",
            apply_clone.ret_ty
        );
    }
}

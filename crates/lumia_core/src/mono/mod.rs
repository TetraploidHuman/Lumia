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
            mono_of: None,
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
            .find(|f| f.name.contains("apply") && f.is_mono_clone())
            .expect("apply mono clone");
        assert_eq!(
            apply_clone.mono_of.as_deref(),
            Some("apply"),
            "mono_of should name the original"
        );
        assert!(
            matches!(apply_clone.ret_ty, Type::Float),
            "apply clone ret_ty should be Float after refresh, got {:?}",
            apply_clone.ret_ty
        );
    }

    /// Transitive FunRef HOF needs multiple clone rounds but must converge
    /// well under [`super::specialize::`]-documented `MAX_MONO_CLONE_ROUNDS`.
    #[test]
    fn mono_hof_chain_converges_with_few_clones() {
        let core = compile_source_to_core(
            r#"
module M
type Option { Some(value) None }
val dbl = { x -> x + x }
val optMap = { o, f ->
    o match {
        None -> None
        Some(v) -> Some(f(v))
    }
}
val main = {
    optMap(Some(1.5), dbl)
}
"#,
        )
        .expect("core");
        let mono_clones = core.functions.iter().filter(|f| f.is_mono_clone()).count();
        assert!(
            (1..8).contains(&mono_clones),
            "expected a small number of mono clones, got {mono_clones}: {:?}",
            core.functions
                .iter()
                .filter(|f| f.is_mono_clone())
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        assert!(
            core.functions
                .iter()
                .any(|f| f.name.contains("dbl") && f.name.contains("$Float")),
            "expected transitive dbl$Float clone"
        );
    }

    #[test]
    fn specialize_clones_map_get_key_mangling() {
        let core = compile_source_to_core(
            r#"
module M
val id = { m -> m }
val main = {
    id(mapOf(1 to 2))
    id(mapOf("a" to 3))
}
"#,
        )
        .expect("core");
        let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names.contains(&"id$Map_Int_Int"),
            "expected exact id$Map_Int_Int, funs={names:?}"
        );
        assert!(
            names.contains(&"id$Map_String_Int"),
            "expected exact id$Map_String_Int, funs={names:?}"
        );
    }

    #[test]
    fn specialize_clones_set_id_key_mangling() {
        let core = compile_source_to_core(
            r#"
module M
val id = { s -> s }
val main = {
    id(setOf(1, 2))
    id(setOf("a", "b"))
}
"#,
        )
        .expect("core");
        let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names.contains(&"id$Set_Int"),
            "expected exact id$Set_Int, funs={names:?}"
        );
        assert!(
            names.contains(&"id$Set_String"),
            "expected exact id$Set_String, funs={names:?}"
        );
    }

    #[test]
    fn mono_key_suffix_map_set() {
        assert_eq!(
            MonoKey(vec![MonoKind::Map(
                Box::new(MonoKind::Int),
                Box::new(MonoKind::Float)
            )])
            .suffix(),
            "$Map_Int_Float"
        );
        assert_eq!(
            MonoKey(vec![MonoKind::Set(Box::new(MonoKind::String))]).suffix(),
            "$Set_String"
        );
    }

    #[test]
    fn specialize_option_map_funref_rounds() {
        let core = compile_source_to_core(
            r#"
module M
type Option { Some(value) None }
val optMap = { opt, f ->
    opt match {
        None -> None
        Some(x) -> Some(f(x))
    }
}
val dbl = { x -> x + x }
val main = {
    optMap(Some(1), dbl)
    optMap(Some(1.5), dbl)
}
"#,
        )
        .expect("core");
        let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names
                .iter()
                .any(|n| n.contains("optMap") && n.contains('$')),
            "expected optMap$* mono clones, funs={names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| n.contains("dbl") && n.contains("$Float")),
            "expected dbl$Float for Float Option path, funs={names:?}"
        );
    }

    #[test]
    fn specialize_hof_funref_directizes_to_float_call() {
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
            .find(|f| f.name.starts_with("apply$") && f.name.contains("Fn_dbl"))
            .unwrap_or_else(|| {
                panic!(
                    "expected apply$*_Fn_dbl clone, funs={:?}",
                    core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
                )
            });
        assert!(
            matches!(apply_clone.ret_ty, Type::Float),
            "apply FunRef clone ret_ty should be Float, got {:?}",
            apply_clone.ret_ty
        );
        assert!(
            core.functions.iter().any(|f| f.name == "dbl$Float"),
            "second-round Float clone missing, funs={:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
        assert!(
            crate::block_calls(&apply_clone.body, "dbl$Float"),
            "FunRef should directize to Call(dbl$Float); body={:?}",
            apply_clone.body
        );
    }
}

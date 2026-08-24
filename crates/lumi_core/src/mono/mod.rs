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
    use crate::ir::{Block, CoreFun, Local, Op, Value};
    use lumi_ty::{Effect, Type};
    #[test]
    fn args_mono_key_prefers_formal_adt_over_abi_int() {
        use super::key::args_mono_key;
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
        let tys = super::key::materialize_mono_param_tys(&key, &formals, &[]);
        assert!(matches!(
            &tys[0],
            Type::Adt { name, params } if name == "Parts" && params.len() == 2
        ));
        assert!(matches!(tys[1], Type::Float));
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
    fn specialize_clones_poly_list_float_from_var_slot() {
        // Call-site `var xs = listOf(floats)` loads as `Name(xs)`. Mono must
        // track slot types so `List[Float]` specializes (else ListGet stays Int
        // and println shows IEEE bit patterns).
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val nearest = { pts, n ->
    println(pts.get(0))
    pts.get(0) + pts.get(1)
}
val main = {
    var xs = listOf(0.668, 0.460)
    println(xs.get(0))
    println(nearest(xs, 2))
}
"#,
        )
        .expect("core");
        let clone = core
            .functions
            .iter()
            .find(|f| f.name.contains("nearest") && f.is_mono_clone())
            .unwrap_or_else(|| {
                panic!(
                    "expected nearest$List_Float clone, funs={:?}",
                    core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
                )
            });
        assert!(
            matches!(
                clone.param_tys.first(),
                Some(Type::List(e)) if matches!(e.as_ref(), Type::Float)
            ),
            "clone param0 should be List[Float], got {:?}",
            clone.param_tys
        );
        assert!(
            matches!(clone.ret_ty, Type::Float),
            "clone ret should be Float from ListGet+add, got {:?}",
            clone.ret_ty
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

    /// Heap products typed as Int at call sites must not poison mono: either no
    /// Int-erased clone, or any clone keeps structural `Parts` params so
    /// `steps + 1.0` bitcasts IEEE bits (not sitofp).
    #[test]
    fn mono_preserves_adt_param_when_call_site_erases_to_int() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
type Parts {
    val a
    val b
    val c
    val d
    val e
    val f
    val g
    val h
    val i
    val j
    val k
    val l
    val m
    val n
    val o
    val p
    val q
    val r
    val s
    val t
    val u
    val v
    val w
    val x
    val y
    val z
    val aa
    val ab
    val ac
    val ad
    val ae
    val af
    val ag
    val ah
    val ai
    val aj
    val steps
}
val bump = { p0, reward ->
    Parts {
        a = p0.a, b = p0.b, c = p0.c, d = p0.d, e = p0.e, f = p0.f, g = p0.g, h = p0.h,
        i = p0.i, j = p0.j, k = p0.k, l = p0.l, m = p0.m, n = p0.n, o = p0.o, p = p0.p,
        q = p0.q, r = p0.r, s = p0.s, t = p0.t, u = p0.u, v = p0.v, w = p0.w, x = p0.x,
        y = p0.y, z = p0.z, aa = p0.aa, ab = p0.ab, ac = p0.ac, ad = p0.ad, ae = p0.ae,
        af = p0.af, ag = p0.ag, ah = p0.ah, ai = p0.ai, aj = p0.aj,
        steps = p0.steps + 1.0 + reward
    }
}
val main = {
    var p = Parts {
        a = 0, b = 0, c = 0, d = 0, e = 0, f = 0, g = 0, h = 0,
        i = 0, j = 0, k = 0, l = 0, m = 0, n = 0, o = 0, p = 0,
        q = 0, r = 0, s = 0, t = 0, u = 0, v = 0, w = 0, x = 0,
        y = 0, z = 0, aa = 0, ab = 0, ac = 0, ad = 0, ae = 0,
        af = 0, ag = 0, ah = 0, ai = 0, aj = 0,
        steps = 0.0
    }
    p = bump(p, 0.0 - 0.01)
    p = bump(p, 0.0 - 0.01)
    println(p.steps)
}
"#,
        )
        .expect("core");
        for f in &core.functions {
            if let Some(rest) = f.name.strip_prefix("bump$") {
                assert!(
                    !rest.starts_with("Int"),
                    "must not mono-key ABI-erased product as Int: {}",
                    f.name
                );
                let p0 = f.param_tys.first().expect("p0");
                assert!(
                    matches!(p0, Type::Adt { name, params } if name == "Parts" && !params.is_empty()),
                    "mono clone {} must keep Parts field params, got {:?}",
                    f.name,
                    p0
                );
            }
        }
    }

    /// Shared denom `n` across product float fields must not sticky-`seen` to Int.
    #[test]
    fn product_shared_float_div_fields_keep_float_params() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
type Eco { val ecoX }
type Parts { val pX }
type Roll {
    val rollEco
    val rollParts
    val meanFood
    val meanThreat
    val meanDisp
}
val rollout = { eco0, parts0, nEp ->
    var eco = eco0
    var parts = parts0
    var sumFood = 0.0
    var sumThreat = 0.0
    var sumDisp = 0.0
    var ep = 0
    for ep < nEp {
        sumFood = sumFood + 1.0
        sumThreat = sumThreat + 15.2
        sumDisp = sumDisp + 3.5
        ep = ep + 1
    }
    val n = 0.0 + nEp
    Roll {
        rollEco = eco,
        rollParts = parts,
        meanFood = sumFood / n,
        meanThreat = sumThreat / n,
        meanDisp = sumDisp / n
    }
}
val main = {
    val eco = Eco { ecoX = 1 }
    val parts = Parts { pX = 2 }
    val r = rollout(eco, parts, 2)
    println(r.meanFood)
    println(r.meanThreat)
    println(r.meanDisp)
}
"#,
        )
        .expect("core");
        let rollout = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("rollout$"))
            .or_else(|| core.functions.iter().find(|f| f.name == "rollout"))
            .expect("rollout");
        match &rollout.ret_ty {
            Type::Adt { name, params } => {
                assert_eq!(name, "Roll");
                assert_eq!(params.len(), 5, "Roll params arity, got {:?}", params);
                assert!(
                    matches!(params[2], Type::Float)
                        && matches!(params[3], Type::Float)
                        && matches!(params[4], Type::Float),
                    "mean* fields must be Float, got {:?}",
                    params
                );
            }
            other => panic!("expected Roll Adt ret_ty, got {other:?}"),
        }
    }

    #[test]
    fn specialize_list_par_map_float_callback() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val main = {
    val xs = listOf(1.5, 2.5).map({ x -> x + x })
    println(xs.get(0))
}
"#,
        )
        .expect("core");
        let float_lam = core.functions.iter().find(|f| {
            f.name.starts_with("__lam_")
                && f.name.contains("$Float")
                && f.param_tys
                    .first()
                    .is_some_and(|t| matches!(t, Type::Float))
        });
        assert!(
            float_lam.is_some(),
            "expected __lam_*$Float with Float param, funs={:?}",
            core.functions
                .iter()
                .map(|f| (&f.name, &f.param_tys, &f.ret_ty))
                .collect::<Vec<_>>()
        );
        // Call site FunRef must point at the Float clone (not the Int ABI stub).
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let mut saw = false;
        crate::for_each_block_dfs(&main.body, &mut |b| {
            for op in &b.ops {
                if let Op::Let {
                    value: Value::FunRef(n),
                    ..
                } = op
                {
                    if n.contains("__lam_") && n.contains("$Float") {
                        saw = true;
                    }
                }
            }
        });
        assert!(saw, "main should FunRef the Float mono clone");
    }

    #[test]
    fn specialize_fused_map_fold_float_add() {
        let core = compile_source_to_core(
            r#"
module MapFold
import std.io.{println}
val dbl = { x -> x + x }
val add = { a, b -> a + b }
val main = {
    val s = listOf(1.5, 2.5).map(dbl).fold(0.0, add)
    println(s == 8.0)
}
"#,
        )
        .expect("core");
        let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names
                .iter()
                .any(|n| *n == "add$Float" || *n == "add$Float_Float"),
            "expected add$Float(_Float), funs={names:?}"
        );
        assert!(
            !names.iter().any(|n| n.contains("add$Float_Int")),
            "must not create add$Float_Int, funs={names:?}"
        );
    }

    #[test]
    fn specialize_list_par_fold_float_add() {
        let core = compile_source_to_core(
            r#"
module M
import std.io.{println}
val dbl = { x -> x + x }
val add = { a, b -> a + b }
val main = {
    val xs = listOf(1.5, 2.5).map(dbl)
    val s = xs.fold(0.0, add)
    println(s)
}
"#,
        )
        .expect("core");
        let names: Vec<_> = core.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names
                .iter()
                .any(|n| n.contains("add") && n.contains("Float")),
            "expected add$Float_* clone, funs={names:?}"
        );
        let main = core.functions.iter().find(|f| f.name == "main").unwrap();
        let mut fold_funref = None;
        crate::for_each_block_dfs(&main.body, &mut |b| {
            for op in &b.ops {
                if let Op::Let {
                    value:
                        Value::Builtin {
                            name: lumi_hir::Builtin::ListParFold,
                            args,
                        },
                    ..
                } = op
                {
                    if let Some(cb) = args.get(2) {
                        for op2 in &b.ops {
                            if let Op::Let {
                                local,
                                value: Value::FunRef(n),
                                ..
                            } = op2
                            {
                                if local.0 == cb.0 {
                                    fold_funref = Some(n.clone());
                                }
                            }
                        }
                    }
                }
            }
        });
        let fr = fold_funref.expect("ListParFold funref");
        assert!(
            fr.contains("Float"),
            "ListParFold should use add$Float_*, got {fr}"
        );
    }

    #[test]
    fn andthen_float_payload_ret_tys() {
        let core = compile_source_to_core(
            r#"
module M
type Option { Some(v) None }
val andThen = { o, f ->
    o match {
        None -> None
        Some(x) -> f(x)
    }
}
val times2 = { x -> Some(x * 2.0) }
val main = {
    andThen(Some(1.5), times2) match {
        Some(v) -> v
        None -> 0.0
    }
}
"#,
        )
        .expect("core");
        let times2 = core
            .functions
            .iter()
            .find(|f| f.name == "times2")
            .expect("times2");
        assert!(
            matches!(
                &times2.ret_ty,
                Type::Adt { name, params } if name == "Option"
                    && params.first().is_some_and(|p| matches!(p, Type::Float))
            ),
            "times2 ret should be Option[Float], got {:?}",
            times2.ret_ty
        );
        let and_then = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("andThen$"))
            .expect("andThen mono clone");
        assert!(
            matches!(
                &and_then.ret_ty,
                Type::Adt { name, params } if name == "Option"
                    && params.first().is_some_and(|p| matches!(p, Type::Float))
            ),
            "andThen$ clone ret should be Option[Float], got {:?}",
            and_then.ret_ty
        );
        assert!(
            matches!(
                and_then.param_tys.first(),
                Some(Type::Adt { name, params }) if name == "Option"
                    && params.first().is_some_and(|p| matches!(p, Type::Float))
            ),
            "andThen$ param0 should be Option[Float], got {:?}",
            and_then.param_tys
        );
    }
}

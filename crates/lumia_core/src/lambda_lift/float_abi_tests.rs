use super::is_apply_hof;
use crate::compile_source_to_core;

#[test]
fn spawn_string_cap_closure_ret_is_string() {
    let str2 = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val prefix = "pre"
        val f = spawn { { s -> prefix.concat(s) } }.join()
        println(f("x").len())
    }
}
"#,
    )
    .expect("str2");
    let lam0 = str2
        .functions
        .iter()
        .find(|f| f.name == "__lam_0")
        .expect("__lam_0");
    assert!(
        matches!(lam0.ret_ty, lumia_ty::Type::String),
        "spawned concat lam ret should be String, got {:?}",
        lam0.ret_ty
    );
}

#[test]
fn spawn_some_true_option_bool() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val o = spawn { Some(true) }.join()
        println(o)
    }
}
"#,
    )
    .expect("core");
    let lam = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn lam");
    assert!(
        matches!(
            &lam.ret_ty,
            lumia_ty::Type::Adt { name, params }
                if lumia_hir::is_option(name) && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Bool))
        ),
        "spawn Some(true) ret should be Option[Bool], got {:?}",
        lam.ret_ty
    );
}

#[test]
fn spawn_two_float_folds_sum_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val xs = listOf(1.0, 2.0, 3.0)
        val ys = listOf(1.0, 2.0)
        val s = spawn { xs.fold(0.0, { a, b -> a + b }) + ys.fold(0.0, { a, b -> a + b }) }.join()
        println(s)
    }
}
"#,
    )
    .expect("core");
    assert!(
        core.functions.iter().any(|f| f.name.starts_with("__lam_")
            && matches!(f.ret_ty, lumia_ty::Type::Float)
            && f.params.len() <= 1),
        "spawn body should return Float"
    );
}

#[test]
fn detect_apply_hof_shape() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val apply = { f, x -> f(x) }
        println(spawn { apply({ y -> y * 2.0 }, 1.5) }.join())
    }
}
"#,
    )
    .expect("core");
    let apply = core
        .functions
        .iter()
        .find(|f| f.name == "__lam_0")
        .expect("apply");
    assert!(is_apply_hof(&apply.params, &apply.body));
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name == "__lam_2")
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "got {:?}",
        spawn.ret_ty
    );
}

#[test]
fn detect_compose_hof_float() {
    use super::is_compose_hof;
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val andThen = { f, g, x -> g(f(x)) }
        println(spawn { andThen({ a -> a + 1.0 }, { b -> b * 2.0 }, 1.5) }.join())
    }
}
"#,
    )
    .expect("core");
    let compose = core
        .functions
        .iter()
        .find(|f| is_compose_hof(&f.params, &f.body))
        .expect("compose");
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.params.len() == 1
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::ir::Op::Let {
                            value: crate::ir::Value::IndirectCall { args, .. },
                            ..
                        } if args.len() == 3
                    ) || matches!(
                        op,
                        crate::ir::Op::Let {
                            value: crate::ir::Value::Call { args, .. },
                            ..
                        } if args.len() == 3
                    )
                })
        })
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "compose spawn ret {:?}, compose={}",
        spawn.ret_ty,
        compose.name
    );
}

#[test]
fn detect_id_through_apply_float() {
    use super::is_id_hof;
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val apply = { f, x -> f(x) }
        val id = { f -> f }
        println(spawn { apply(id({ y -> y * 2.0 }), 1.5) }.join())
    }
}
"#,
    )
    .expect("core");
    assert!(core.functions.iter().any(|f| is_id_hof(&f.params, &f.body)));
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.params.len() == 1
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::ir::Op::Let {
                            value: crate::ir::Value::IndirectCall { args, .. },
                            ..
                        } if args.len() == 2
                    ) || matches!(
                        op,
                        crate::ir::Op::Let {
                            value: crate::ir::Value::Call { args, .. },
                            ..
                        } if args.len() == 2
                    )
                })
        })
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "id∘apply spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_map_get_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { listOf(1.5, 2.5).map({ x -> x * 2.0 }).get(0) }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "map.get spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_if_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val t = spawn { if true { 1.5 } else { 2.5 } }
        println(t.join() * 2.0)
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "if-float spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_fold_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { listOf(1.5, 2.5).fold(0.0, { a, b -> a + b }) }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "fold-float spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_filter_get_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { listOf(1.5, 2.5).filter({ x -> x > 2.0 }).get(0) }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "filter.get spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_filter_list_float_keeps_list_ret() {
    // Regression: `__flt_acc_*` was mis-named `__filter_acc` in float_cap_fixup.
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ys = spawn { listOf(1.0, 2.0, -1.0).filter { x -> x > 0.0 } }.join()
        println(ys.len)
        println(ys.get(0))
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::List(t) if matches!(t.as_ref(), lumia_ty::Type::Float)
        ),
        "filter list spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_mut_list_float_acc_keeps_list_ret() {
    // User `var acc = listOf(1.0); …; acc` shares Name+Elems with scalar folds;
    // must not upgrade List → Float.
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ys = spawn {
            var acc = listOf(1.0)
            for x in listOf(2.0, 3.0) { acc = acc.append(x) }
            acc
        }.join()
        println(ys.len)
        println(ys.get(0))
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::List(t) if matches!(t.as_ref(), lumia_ty::Type::Float)
        ),
        "mut list acc spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_sort_by_list_float_keeps_list_ret() {
    // ListSortByKeys was missing from local_heap_ty → soft List[Int] across join.
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ys = spawn {
            listOf(3.0, 1.0, 2.0).sortBy { x -> 0 }
        }.join()
        println(ys.len)
        println(ys.get(0))
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::List(t) if matches!(t.as_ref(), lumia_ty::Type::Float)
        ),
        "sortBy list spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_map_items_float_keeps_tuple_val() {
    // MapItems was missing from local_heap_ty → List[Int] across join.
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val xs = spawn { mapOf(1, 1.5).items() }.join()
        println(xs.get(0).1)
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    let ok = match &spawn.ret_ty {
        lumia_ty::Type::List(e) => match e.as_ref() {
            lumia_ty::Type::Adt { name, params } if name == "__Tuple" && params.len() == 2 => {
                matches!(params[1], lumia_ty::Type::Float)
            }
            _ => false,
        },
        _ => false,
    };
    assert!(ok, "map items spawn ret {:?}", spawn.ret_ty);
}

#[test]

fn spawn_toset_list_float_keeps_set_ret() {
    // SetInsert missing → empty setOf/toSet stuck as Set[Int].
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    val xs = scope {
        spawn { listOf(1.5, 2.5).toSet() }.join()
    }.toList()
    println(xs.get(0))
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::Set(t) if matches!(t.as_ref(), lumia_ty::Type::Float)
        ),
        "toSet spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_ret_channel_float_keeps_channel_ret() {
    // ChannelNew missing from heap/channel refresh → soft List[Int]; recv printed bits.
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = spawn {
            val c = channel(1)
            c.send(1.5)
            c
        }.join()
        println(ch.recv())
        ch.close()
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::Channel(t) if matches!(t.as_ref(), lumia_ty::Type::Float)
        ),
        "channel spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_set_remove_float_keeps_set_ret() {
    // MapRemove lacked Set arm → Set.remove typed as Map in ABI walkers.
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val s = spawn { setOf(1.5, 2.5).remove(0.0) }.join()
        val xs = s.toList()
        println(xs.get(0))
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::Set(t) if matches!(t.as_ref(), lumia_ty::Type::Float)
        ),
        "set.remove spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_match_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            Some(1.5) match {
                Some(x) -> x * 2.0
                None -> 0.0
            }
        }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "match-float spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_match_id_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            Some(1.5) match {
                Some(x) -> x
                None -> 0.0
            }
        }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "match-id-float spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_match_option_float_keeps_adt_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val o = spawn {
            Some(1.5) match {
                Some(x) -> Some(x * 2.0)
                None -> None
            }
        }.join()
        o match {
            Some(v) -> println(v)
            None -> println(0)
        }
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::Adt { name, params }
                if lumia_hir::is_option(name)
                    && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Float))
        ),
        "match-option-float spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_list_option_float_keeps_elem_adt() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val xs = spawn { listOf(Some(1.5)) }.join()
        xs.get(0) match {
            Some(v) -> println(v * 2.0)
            None -> println(0)
        }
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::List(e)
                if matches!(
                    e.as_ref(),
                    lumia_ty::Type::Adt { name, params }
                        if lumia_hir::is_option(name)
                            && params.first().is_some_and(|p| matches!(p, lumia_ty::Type::Float))
                )
        ),
        "list-option-float spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_for_float_acc_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            var s = 0.0
            for x in listOf(1.0, 2.0, 3.0) { s = s + x }
            s
        }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "for-float-acc spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_mut_fun_call_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            var f = { x -> x * 2.0 }
            f(1.5)
        }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Float),
        "mut-fun-call spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_bool_cmp_keeps_int_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn { 1.5 > 1.0 }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Bool),
        "bool-cmp spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_mut_bool_keeps_bool_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        println(spawn {
            var b = false
            b = 1.5 > 1.0
            b
        }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn");
    assert!(
        matches!(spawn.ret_ty, lumia_ty::Type::Bool),
        "mut-bool spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn spawn_nested_task_keeps_task_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val outer = spawn { spawn { 1.5 * 2.0 } }
        println(outer.join().join())
    }
}
"#,
    )
    .expect("core");
    let outer = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.params.is_empty()
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::ir::Op::Let {
                            value: crate::ir::Value::Builtin {
                                name: lumia_hir::Builtin::TaskSpawn,
                                ..
                            },
                            ..
                        }
                    )
                })
        })
        .expect("outer spawn");
    assert!(
        matches!(
            &outer.ret_ty,
            lumia_ty::Type::Task(e) if matches!(e.as_ref(), lumia_ty::Type::Float)
        ),
        "nested spawn ret should be Task[Float], got {:?}",
        outer.ret_ty
    );
}

#[test]
fn map_spawn_join_list_append_task_float() {
    use crate::value_ty::{infer_value_ty_ctx, CodegenTypeTables, InferValueCtx};
    use crate::Op;
    use lumia_ty::Type;
    use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val xs = listOf(1.5, 2.5).map({ x -> spawn { x * 2.0 } })
        println(xs.get(0).join())
    }
}
"#,
    )
    .expect("core");
    let tables = crate::ModuleTables::from_module(&core);
    let fun_ret_tys = &tables.fun_ret_tys;
    let fun_param_tys = &tables.fun_param_tys;
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let mut local_tys: HashMap<u32, Type> = HashMap::default();
    let mut slot_tys: HashMap<lumia_syntax::Sym, Type> = HashMap::default();
    let fun_param0_identity = HashSet::default();
    let mut funref_locals: HashMap<u32, String> = HashMap::default();
    let local_int_consts: HashMap<u32, i64> = HashMap::default();
    let sum_max_arity: HashMap<String, usize> = HashMap::default();
    fn walk(
        ops: &[Op],
        local_tys: &mut HashMap<u32, Type>,
        slot_tys: &mut HashMap<lumia_syntax::Sym, Type>,
        funref_locals: &mut HashMap<u32, String>,
        fun_ret_tys: &HashMap<String, Type>,
        fun_param_tys: &HashMap<String, Vec<Type>>,
        fun_param0_identity: &HashSet<String>,
        local_int_consts: &HashMap<u32, i64>,
        sum_max_arity: &HashMap<String, usize>,
        hint: Option<&Type>,
    ) {
        for op in ops {
            match op {
                Op::Let { local, value, .. } => {
                    crate::for_each_nested_block(value, &mut |b| {
                        walk(
                            &b.ops,
                            local_tys,
                            slot_tys,
                            funref_locals,
                            fun_ret_tys,
                            fun_param_tys,
                            fun_param0_identity,
                            local_int_consts,
                            sum_max_arity,
                            hint,
                        );
                    });
                    let ty = infer_value_ty_ctx(
                        value,
                        InferValueCtx::full(
                            local_tys,
                            CodegenTypeTables {
                                slot_tys,
                                fun_ret_tys,
                                fun_param_tys,
                                fun_param0_identity,
                                funref_locals,
                                local_int_consts,
                                sum_max_arity,
                                channel_elem_hint: hint,
                            },
                        ),
                        None,
                    );
                    local_tys.insert(local.0, ty);
                }
                Op::Assign { name, value } => {
                    if let Some(ty) = local_tys.get(&value.0).cloned() {
                        slot_tys.insert(name.clone(), ty);
                    }
                }
                _ => {}
            }
        }
    }
    walk(
        &main.body.ops,
        &mut local_tys,
        &mut slot_tys,
        &mut funref_locals,
        fun_ret_tys,
        fun_param_tys,
        &fun_param0_identity,
        &local_int_consts,
        &sum_max_arity,
        core.channel_elem_hint.as_ref(),
    );
    let join_ty = local_tys
        .values()
        .find(|t| matches!(t, Type::Float))
        .cloned();
    assert!(
        join_ty.is_some(),
        "expected Float join result in local_tys, got {:?}",
        local_tys
            .iter()
            .filter(|(_, t)| matches!(t, Type::Task(_) | Type::List(_) | Type::Float))
            .collect::<Vec<_>>()
    );
    assert!(
        local_tys
            .values()
            .any(|t| matches!(t, Type::List(e) if matches!(e.as_ref(), Type::Task(_)))),
        "map acc should become List[Task[_]], slot_tys={slot_tys:?}"
    );
}

#[test]
fn flatmap_list_fun_println_ty() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    val xs = listOf(1.0, 2.0)
    val fs = xs.flatMap({ x -> listOf({ y -> x + y }) })
    println(fs.get(0)(1.0))
}
"#,
    )
    .expect("core");
    for f in &core.functions {
        if f.name == "main" || f.name.starts_with("__lam") {
            eprintln!("{} ret={:?} params={:?}", f.name, f.ret_ty, f.param_tys);
        }
    }
    // Check main has Call to $Float or Float println path via funref
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    for op in &main.body.ops {
        if let crate::Op::Let { local, value, .. } = op {
            match value {
                crate::Value::Call { fun, args } => {
                    eprintln!("  %{} Call {} {:?}", local.0, fun, args)
                }
                crate::Value::IndirectCall { callee, args } => {
                    eprintln!("  %{} ICall %{} {:?}", local.0, callee.0, args)
                }
                crate::Value::Builtin {
                    name: lumia_hir::Builtin::ListGet,
                    args,
                    ..
                } => eprintln!("  %{} ListGet {:?}", local.0, args),
                crate::Value::Builtin {
                    name: lumia_hir::Builtin::ListConcat,
                    args,
                    ..
                } => eprintln!("  %{} ListConcat {:?}", local.0, args),
                crate::Value::FunRef(n) => eprintln!("  %{} FunRef {}", local.0, n),
                crate::Value::AllocClosure { fun, .. } => {
                    eprintln!("  %{} AllocClosure {}", local.0, fun)
                }
                _ => {}
            }
        }
    }
}

#[test]
fn spawn_list_of_float_fun_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val fs = spawn { listOf({ x -> x * 2.0 }) }.join()
        println(fs.get(0)(1.5))
    }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn_lam");
    assert!(
        matches!(
            &spawn.ret_ty,
            lumia_ty::Type::List(e) if matches!(e.as_ref(), lumia_ty::Type::Fun(ps, r, _) if ps.len() == 1 && matches!(ps[0], lumia_ty::Type::Float) && matches!(r.as_ref(), lumia_ty::Type::Float))
        ),
        "got {:?}",
        spawn.ret_ty
    );
}

use super::refine_channel_elem_hint;
use crate::compile_source_to_core;
use lumia_ty::Type;

#[test]
fn spawn_join_list_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val t = spawn { listOf(1.5, 2.5) }
        val xs = t.join()
        println(xs.get(0) * 2.0)
    }
}
"#,
    )
    .expect("core");
    let lam = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_"))
        .expect("lam");
    assert!(
        matches!(&lam.ret_ty, Type::List(e) if matches!(e.as_ref(), Type::Float)),
        "spawn lambda ret should be List[Float], got {:?}",
        lam.ret_ty
    );
}

#[test]
fn named_fun_send_via_spawn_keeps_fun_hint() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(1)
        val f = { x -> x * 2.0 }
        spawn { ch.send(f) }
        val g = ch.recv()
        println(g(1.5))
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    assert!(
        core.channel_elem_conflicts.is_empty(),
        "named Fun send should not conflict: {:?}",
        core.channel_elem_conflicts
    );
    let hint = core
        .channel_elem_by_local
        .values()
        .next()
        .cloned()
        .or(core.channel_elem_hint.clone());
    assert!(
        matches!(&hint, Some(Type::Fun(_, ret, _)) if matches!(ret.as_ref(), Type::Float)),
        "expected Channel[Fun→Float] hint, got {hint:?}"
    );
}

#[test]
fn channel_new_stamps_ground_elem_from_type_at() {
    use crate::visit::for_each_block_dfs;
    use crate::{Op, Value};
    use lumia_hir::Builtin;

    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(1)
        spawn { ch.send(1.5) }
        println(ch.recv() * 2.0)
    }
}
"#,
    )
    .expect("core");
    let mut found = false;
    for fun in &core.functions {
        for_each_block_dfs(&fun.body, &mut |block| {
            for op in &block.ops {
                if let Op::Let {
                    value:
                        Value::Builtin {
                            name: Builtin::ChannelNew,
                            result_ty: Some(Type::Channel(e)),
                            ..
                        },
                    ..
                } = op
                {
                    assert!(
                        matches!(e.as_ref(), Type::Float),
                        "ChannelNew stamp should be Channel[Float], got {:?}",
                        e
                    );
                    found = true;
                }
            }
        });
    }
    assert!(found, "expected stamped ChannelNew from type_at");
    assert!(
        matches!(&core.channel_elem_hint, Some(Type::Float)),
        "module hint should seed from stamp, got {:?}",
        core.channel_elem_hint
    );
}

#[test]
fn channel_send_list_float_sets_hint() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(1)
        spawn { ch.send(listOf(1.5, 2.5)) }
        val xs = ch.recv()
        println(xs.get(0) * 2.0)
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    assert!(
        matches!(
            &core.channel_elem_hint,
            Some(Type::List(e)) if matches!(e.as_ref(), Type::Float)
        ),
        "expected List[Float] hint, got {:?}",
        core.channel_elem_hint
    );
}

#[test]
fn two_channels_keep_distinct_elem_hints() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val a = channel(1)
        val b = channel(1)
        spawn {
            a.send(1.5)
            b.send(listOf(2.5))
        }
        println(a.recv() * 2.0)
        println(b.recv().get(0) * 2.0)
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    assert!(
        core.channel_elem_hint.is_none(),
        "mixed channels must not set module hint"
    );
    let tys: Vec<_> = core.channel_elem_by_local.values().cloned().collect();
    assert!(
        tys.iter().any(|t| matches!(t, Type::Float)),
        "expected a Float channel, got {:?}",
        core.channel_elem_by_local
    );
    assert!(
        tys.iter()
            .any(|t| matches!(t, Type::List(e) if matches!(e.as_ref(), Type::Float))),
        "expected a List[Float] channel, got {:?}",
        core.channel_elem_by_local
    );
}

#[test]
fn spawn_join_map_keeps_map_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val m = spawn { mapOf(1 to 2) }.join()
        println(m.get(1))
    }
}
"#,
    )
    .expect("core");
    let lam = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_"))
        .expect("lam");
    assert!(
        matches!(&lam.ret_ty, Type::Map(_, _)),
        "spawn lambda ret should be Map, got {:?}",
        lam.ret_ty
    );
}

#[test]
fn spawn_join_set_keeps_set_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val s = spawn { setOf(1, 2) }.join()
        println(s.contains(1))
    }
}
"#,
    )
    .expect("core");
    let lam = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_"))
        .expect("lam");
    assert!(
        matches!(&lam.ret_ty, Type::Set(_)),
        "spawn lambda ret should be Set, got {:?}",
        lam.ret_ty
    );
}

#[test]
fn spawn_call_float_fun_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val dbl = { x -> x * 2.0 }
val main = {
    scope {
        println(spawn { dbl(1.5) }.join())
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
        matches!(lam.ret_ty, Type::Float),
        "spawn {{ dbl(1.5) }} ret should be Float, got {:?}",
        lam.ret_ty
    );
}

#[test]
fn spawn_local_closure_float_keeps_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val dbl = { x -> x * 2.0 }
        println(spawn { dbl(1.5) }.join())
    }
}
"#,
    )
    .expect("core");
    let spawn_lam = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::ir::Op::Let {
                            value: crate::ir::Value::IndirectCall { .. }
                                | crate::ir::Value::Call { .. },
                            ..
                        }
                    )
                })
        })
        .expect("spawn lam with call/icall");
    assert!(
        matches!(spawn_lam.ret_ty, Type::Float),
        "spawn {{ local dbl(1.5) }} ret should be Float, got {:?}",
        spawn_lam.ret_ty
    );
}

#[test]
fn spawn_apply_hof_float_keeps_float_ret() {
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
    let spawn_lam = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.params.len() == 1
                && f.body.ops.iter().any(|op| match op {
                    crate::ir::Op::Let {
                        value: crate::ir::Value::IndirectCall { args, .. },
                        ..
                    } => args.len() == 2,
                    crate::ir::Op::Let {
                        value: crate::ir::Value::Call { args, .. },
                        ..
                    } => args.len() == 2,
                    _ => false,
                })
        })
        .expect("spawn closure lam");
    assert!(
        matches!(spawn_lam.ret_ty, Type::Float),
        "spawn apply-hof ret should be Float, got {:?}",
        spawn_lam.ret_ty
    );
}

#[test]
fn spawn_return_fun_keeps_fun_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val f = spawn { { x -> x * 2.0 } }.join()
        println(f(1.5))
    }
}
"#,
    )
    .expect("core");
    let thunk = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn thunk");
    assert!(
        matches!(&thunk.ret_ty, Type::Fun(ps, r, _) if matches!(r.as_ref(), Type::Float) && ps.iter().any(|p| matches!(p, Type::Float))),
        "spawn returning Fun should be Fun(Float)->Float, got {:?}",
        thunk.ret_ty
    );
}

#[test]
fn channel_send_list_option_float_sets_hint() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(1)
        spawn { ch.send(listOf(Some(1.5))) }
        ch.recv().get(0) match {
            Some(v) -> println(v * 2.0)
            None -> println(0)
        }
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    let hint = core
        .channel_elem_by_local
        .values()
        .next()
        .cloned()
        .or(core.channel_elem_hint.clone());
    assert!(
        matches!(
            &hint,
            Some(Type::List(e))
                if matches!(
                    e.as_ref(),
                    Type::Adt { name, params }
                        if lumia_hir::is_option(name)
                            && params.first().is_some_and(|p| matches!(p, Type::Float))
                )
        ),
        "channel list-option-float hint {:?}",
        hint
    );
}

#[test]
fn channel_send_bool_sets_hint() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(1)
        spawn { ch.send(1.5 > 1.0) }
        println(ch.recv())
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    let hint = core
        .channel_elem_by_local
        .values()
        .next()
        .cloned()
        .or(core.channel_elem_hint.clone());
    assert!(
        matches!(hint, Some(Type::Bool)),
        "channel bool hint {:?}",
        hint
    );
}

#[test]
fn channel_option_some_none_and_result_ok_err_join() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(2)
        spawn {
            ch.send(Some(1.5))
            ch.send(None)
        }
        println(ch.recv() alt 0.0)
        val ch2 = channel(2)
        spawn {
            ch2.send(Ok(1.5))
            ch2.send(Err("e"))
        }
        println(ch2.recv())
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    assert!(
        core.channel_elem_conflicts.is_empty(),
        "Option/Result variants must not conflict: {:?}",
        core.channel_elem_conflicts
    );
    let mut tys: Vec<_> = core.channel_elem_by_local.values().cloned().collect();
    tys.sort_by_key(|t| format!("{t:?}"));
    assert_eq!(tys.len(), 2, "expected two channels, got {tys:?}");
    assert!(
        tys.iter().any(|t| matches!(
            t,
            Type::Adt { name, params }
                if lumia_hir::is_option(name)
                    && params.first().is_some_and(|p| matches!(p, Type::Float))
        )),
        "expected Option[Float], got {tys:?}"
    );
    assert!(
        tys.iter().any(|t| matches!(
            t,
            Type::Adt { name, params }
                if lumia_hir::is_result(name)
                    && params.len() == 2
                    && matches!(params[0], Type::Float)
                    && matches!(params[1], Type::String)
        )),
        "expected Result[Float, String], got {tys:?}"
    );
}

#[test]
fn channel_send_fun_float_sets_hint() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(1)
        spawn { ch.send({ x -> x * 2.0 }) }
        println(ch.recv()(1.5))
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    let hint = core
        .channel_elem_by_local
        .values()
        .next()
        .cloned()
        .or(core.channel_elem_hint.clone());
    assert!(
        matches!(
            &hint,
            Some(Type::Fun(ps, r, _))
                if matches!(r.as_ref(), Type::Float)
                    && ps.iter().any(|p| matches!(p, Type::Float))
        ),
        "channel fun-float hint {:?}",
        hint
    );
}

#[test]
fn channel_send_task_float_sets_hint() {
    let mut core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(1)
        spawn { ch.send(spawn { 1.5 * 2.0 }) }
        println(ch.recv().join())
    }
}
"#,
    )
    .expect("core");
    refine_channel_elem_hint(&mut core);
    let hint = core
        .channel_elem_by_local
        .values()
        .next()
        .cloned()
        .or(core.channel_elem_hint.clone());
    assert!(
        matches!(
            &hint,
            Some(Type::Task(e)) if matches!(e.as_ref(), Type::Float)
        ),
        "channel task-float hint {:?}",
        hint
    );
}

#[test]
fn mixed_channel_int_float_or_string_conflict() {
    // Channel elem is monomorphic: mixed sends fail at typecheck (not only
    // Core hint conflict after poly Channel[α] per use).
    let err_f = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val ch = channel(2)
    spawn { ch.send(1) }
    spawn { ch.send(1.5) }
    println(ch.recv())
  }
}
"#,
    )
    .expect_err("int+float channel");
    assert!(
        err_f.contains("mixed payloads")
            || err_f.contains("channel")
            || err_f.contains("type mismatch")
            || err_f.contains("Float")
            || err_f.contains("Int"),
        "unexpected: {err_f}"
    );

    let err_s = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val ch = channel(2)
    spawn { ch.send(1) }
    spawn { ch.send("x") }
    println(ch.recv())
  }
}
"#,
    )
    .expect_err("int+string channel");
    assert!(
        err_s.contains("mixed payloads")
            || err_s.contains("channel")
            || err_s.contains("type mismatch")
            || err_s.contains("String")
            || err_s.contains("Int"),
        "unexpected: {err_s}"
    );
}

#[test]
fn mixed_channel_payloads_recorded_as_conflict() {
    let err = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val ch = channel(2)
        spawn {
            ch.send(1.5)
            ch.send(listOf(2.5, 2.5))
        }
        println(ch.recv())
    }
}
"#,
    )
    .expect_err("mixed channel payloads should fail");
    assert!(
        err.contains("mixed payloads")
            || err.contains("channel")
            || err.contains("type mismatch")
            || err.contains("List")
            || err.contains("Float"),
        "unexpected error: {err}"
    );
}

#[test]
fn audit_r8_closure_cap_string_concat_len() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  val prefix = "pre"
  val f = { s -> prefix.concat(s) }
  val out = f("fix")
  println(out)
  println(out.len())
}
"#,
    )
    .expect("cap string");
    let lam = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_"))
        .expect("lam");
    assert!(
        matches!(lam.ret_ty, Type::String),
        "closure cap string concat ret {:?}",
        lam.ret_ty
    );

    let spawn = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val f = spawn {
      val prefix = "pre"
      { s -> prefix.concat(s) }
    }.join()
    val out = f("fix")
    println(out.len())
  }
}
"#,
    )
    .expect("spawn cap string");
    let inner = spawn
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.params.len() > 1
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::Builtin {
                                name: lumia_hir::Builtin::ListConcat,
                                ..
                            },
                            ..
                        }
                    )
                })
        })
        .expect("inner concat lam");
    assert!(
        matches!(inner.ret_ty, Type::String),
        "spawn closure string concat ret {:?}",
        inner.ret_ty
    );
}

#[test]
fn audit_r7_spawn_string_len() {
    let lit = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val s = spawn { "hello" }.join()
    println(s.len())
  }
}
"#,
    )
    .expect("lit");
    let lit_spawn = lit
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn string lit");
    assert!(
        matches!(lit_spawn.ret_ty, Type::String),
        "spawn {{ \"hello\" }} ret {:?}",
        lit_spawn.ret_ty
    );

    let concat = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val s = spawn { "hello".concat(" ").concat("world") }.join()
    println(s)
    println(s.len())
  }
}
"#,
    )
    .expect("concat");
    let concat_spawn = concat
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("spawn string concat");
    assert!(
        matches!(concat_spawn.ret_ty, Type::String),
        "spawn string concat ret {:?}",
        concat_spawn.ret_ty
    );
}

#[test]
fn audit_r6_map_values_take_reverse_float() {
    let values = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val vs = spawn { mapOf(1 to 1.5, 2 to 2.5).values() }.join()
    println(vs.get(0) + vs.get(1))
  }
}
"#,
    )
    .expect("values");
    let take = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val m = spawn {
      listOf((1, 1.5), (2, 2.5), (3, 3.5), (4, 4.5))
        .filter({ p -> p.0 > 1 })
        .take(2)
        .toMap()
    }.join()
    println(m.get(2))
  }
}
"#,
    )
    .expect("take");
    let rev = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    println(spawn {
      listOf(1.0, 2.0, 3.0, 4.0).filter({ x -> x > 2.0 }).reverse().get(0)
    }.join())
  }
}
"#,
    )
    .expect("rev");
    let ret = |c: &crate::ir::CoreModule| -> Type {
        c.functions
            .iter()
            .find(|f| f.name.starts_with("__lam_"))
            .map(|f| f.ret_ty.clone())
            .unwrap_or(Type::Int)
    };
    assert!(
        matches!(ret(&values), Type::List(ref e) if matches!(e.as_ref(), Type::Float)),
        "values {:?}",
        ret(&values)
    );
    assert!(
        matches!(ret(&take), Type::Map(_, ref v) if matches!(v.as_ref(), Type::Float)),
        "take {:?}",
        ret(&take)
    );
    assert!(matches!(ret(&rev), Type::Float), "rev {:?}", ret(&rev));
}

#[test]
fn audit_r5_remove_and_filter_tomap() {
    let rem = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val m = spawn { mapOf(1 to 1.5, 2 to 2.5).remove(1) }.join()
    println(m.get(2) alt 0.0)
  }
}
"#,
    )
    .expect("rem");
    let filter = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val m = spawn {
      listOf((1, 1.5), (2, 2.5), (3, 3.5))
        .filter({ p -> p.0 > 1 })
        .toMap()
    }.join()
    println(m.get(2))
  }
}
"#,
    )
    .expect("filter");
    let ret = |c: &crate::ir::CoreModule| -> Type {
        c.functions
            .iter()
            .find(|f| f.name.starts_with("__lam_"))
            .map(|f| f.ret_ty.clone())
            .unwrap_or(Type::Int)
    };
    assert!(
        matches!(ret(&rem), Type::Map(_, ref v) if matches!(v.as_ref(), Type::Float)),
        "remove {:?}",
        ret(&rem)
    );
    assert!(
        matches!(ret(&filter), Type::Map(_, ref v) if matches!(v.as_ref(), Type::Float)),
        "filter {:?}",
        ret(&filter)
    );
}

#[test]
fn audit_r4_tomap_and_mapset_float() {
    let tomap = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val m = spawn {
      listOf((1, 1.5), (2, 2.5)).toMap()
    }.join()
    println(m.get(1))
    println(m.get(2))
  }
}
"#,
    )
    .expect("tomap");
    let mapset = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val m = spawn {
      mapOf(1 to 1.0).set(2, 2.5)
    }.join()
    println(m.get(1))
    println(m.get(2))
  }
}
"#,
    )
    .expect("mapset");
    let mapof = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val m = spawn { mapOf(1 to 1.5, 2 to 2.5) }.join()
    println(m.get(1))
  }
}
"#,
    )
    .expect("mapof");
    let ret = |core: &crate::ir::CoreModule| -> Type {
        core.functions
            .iter()
            .find(|f| f.name.starts_with("__lam_"))
            .map(|f| f.ret_ty.clone())
            .unwrap_or(Type::Int)
    };
    assert!(
        matches!(ret(&mapof), Type::Map(_, ref v) if matches!(v.as_ref(), Type::Float)),
        "mapof {:?}",
        ret(&mapof)
    );
    assert!(
        matches!(ret(&tomap), Type::Map(_, ref v) if matches!(v.as_ref(), Type::Float)),
        "tomap {:?}",
        ret(&tomap)
    );
    assert!(
        matches!(ret(&mapset), Type::Map(_, ref v) if matches!(v.as_ref(), Type::Float)),
        "mapset {:?}",
        ret(&mapset)
    );
}

#[test]
fn audit_r4_curried_compose_float() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val id = { x -> x }
    val compose = { f, g -> { x -> f(g(x)) } }
    println(spawn { compose({ y -> y + 1.0 }, id)(2.5) }.join())
  }
}
"#,
    )
    .expect("core");
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
                            value: crate::ir::Value::IndirectCall { .. },
                            ..
                        }
                    )
                })
        })
        .expect("spawn lam");
    assert!(
        matches!(spawn.ret_ty, Type::Float),
        "curried compose spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn audit_r3_with_pipeline_flatmap_float() {
    let with = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Pt { val x val y }
val main = {
  scope {
    val p = Pt { x = 1.0, y = 2.0 }
    val q = spawn { p with { x = 3.5 } }.join()
    println(q.x)
    println(q.y)
  }
}
"#,
    )
    .expect("with");
    let with_spawn = with
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_"))
        .expect("with spawn");
    assert!(
        matches!(
            &with_spawn.ret_ty,
            Type::Adt { name, params }
                if name == "Pt"
                    && params.len() == 2
                    && matches!(params[0], Type::Float)
                    && matches!(params[1], Type::Float)
        ),
        "with spawn ret {:?}",
        with_spawn.ret_ty
    );

    let pipe = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val ts = listOf(1.0, 2.0).map({ x -> spawn { x + 0.5 } })
    println(spawn { ts.get(0).join() + ts.get(1).join() }.join())
  }
}
"#,
    )
    .expect("pipe");
    assert!(
        pipe.functions.iter().any(|f| {
            f.name.starts_with("__lam_")
                && f.params.len() == 1
                && matches!(f.ret_ty, Type::Float)
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::Builtin {
                                name: lumia_hir::Builtin::TaskJoin,
                                ..
                            },
                            ..
                        }
                    )
                })
        }),
        "pipeline spawn should return Float"
    );

    let flat = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    println(spawn {
      listOf(Some(1.5), None, Some(2.5)).flatMap({ o ->
        o match {
          Some(x) -> listOf(x)
          None -> listOf()
        }
      }).get(1)
    }.join())
  }
}
"#,
    )
    .expect("flat");
    let flat_spawn = flat
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("flatmap spawn");
    assert!(
        matches!(flat_spawn.ret_ty, Type::Float),
        "flatmap get spawn ret {:?}",
        flat_spawn.ret_ty
    );
}

#[test]
fn audit_r2_spawn_go_and_fun_list() {
    let a = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val go = { ->
      val make = { k -> { x -> x * k } }
      make(2.0)(1.5)
    }
    println(spawn { go() }.join())
  }
}
"#,
    )
    .expect("a");
    let spawn_go = a
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.params.len() == 1
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::Call { fun, .. },
                            ..
                        } if fun.starts_with("__lam_")
                    )
                })
        })
        .expect("spawn { go() } lam");
    assert!(
        matches!(spawn_go.ret_ty, lumia_ty::Type::Float),
        "spawn {{ go() }} should return Float, got {:?}",
        spawn_go.ret_ty
    );

    let b = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val fs = spawn {
      listOf(1.0, 2.0).map({ k -> { x -> x + k } })
    }.join()
    println(fs.get(0)(2.0))
    println(fs.get(1)(2.0))
  }
}
"#,
    )
    .expect("b");
    let map_spawn = b
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::Name(n),
                            ..
                        } if n.contains("__map_acc")
                    )
                })
        })
        .expect("map list-of-fun spawn lam");
    assert!(
        matches!(
            &map_spawn.ret_ty,
            lumia_ty::Type::List(e) if matches!(
                e.as_ref(),
                lumia_ty::Type::Fun(ps, r, _)
                    if ps.first().is_some_and(|p| matches!(p, lumia_ty::Type::Float))
                        && matches!(r.as_ref(), lumia_ty::Type::Float)
            )
        ),
        "spawn {{ map → Fun }} should be List[Fun[Float→Float]], got {:?}",
        map_spawn.ret_ty
    );
}

#[test]
fn nested_spawn_join_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = { scope { println(spawn { spawn { 2.0 * 2.0 }.join() }.join()) } }
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
                        crate::Op::Let {
                            value: crate::Value::Builtin {
                                name: lumia_hir::Builtin::TaskJoin,
                                ..
                            },
                            ..
                        }
                    )
                })
        })
        .expect("outer spawn");
    assert!(
        matches!(outer.ret_ty, Type::Float),
        "nested spawn.join ret {:?}",
        outer.ret_ty
    );
}

#[test]
fn spawn_captured_make_float_mono() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val make = { k -> { x -> x * k } }
    println(spawn { make(2.0)(1.5) }.join())
  }
}
"#,
    )
    .expect("core");
    assert!(
        core.functions.iter().any(|f| f.name.contains("$Float")),
        "expected Float mono clone, funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_") && matches!(f.ret_ty, Type::Float) && f.params.len() == 1
        })
        .expect("spawn lam with Float ret");
    assert!(
        matches!(spawn.ret_ty, Type::Float),
        "spawn make(2.0)(1.5) ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn local_thunk_channel_recv_float_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val go = { ->
            val a = channel(1)
            val b = channel(1)
            spawn {
                val x = a.recv()
                b.send(x * 2.0)
            }
            a.send(1.5)
            b.recv()
        }
        println(go())
    }
}
"#,
    )
    .expect("core");
    assert!(
        core.channel_elem_by_local.len() >= 2,
        "both channels need Float hints, got {:?}",
        core.channel_elem_by_local
    );
    assert!(
        core.channel_elem_by_local
            .values()
            .all(|t| matches!(t, Type::Float)),
        "expected Float hints, got {:?}",
        core.channel_elem_by_local
    );
    let go = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_") && f.params.is_empty())
        .expect("go thunk");
    assert!(
        matches!(go.ret_ty, Type::Float),
        "go ret should be Float, got {:?}",
        go.ret_ty
    );
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.params.len() == 1
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::Builtin {
                                name: lumia_hir::Builtin::ChannelSend,
                                ..
                            },
                            ..
                        }
                    )
                })
        })
        .expect("spawn thunk");
    assert!(
        matches!(spawn.ret_ty, Type::Unit),
        "spawn send thunk ret should be Unit, got {:?}",
        spawn.ret_ty
    );
}

#[test]
fn ping_float_channels_get_per_local_hint() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
    scope {
        val a = channel(1)
        val b = channel(1)
        spawn {
            val x = a.recv()
            b.send(x * 2.0)
        }
        a.send(1.5)
        println(b.recv())
    }
}
"#,
    )
    .expect("core");
    assert!(
        core.channel_elem_by_local
            .values()
            .all(|t| matches!(t, Type::Float)),
        "expected Float per-channel hints, got {:?}",
        core.channel_elem_by_local
    );
    assert!(matches!(core.channel_elem_hint.as_ref(), Some(Type::Float)));
}

#[test]
fn audit_r9_spawn_option_float_optionmap() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val optionMap = { opt, f ->
  opt match {
    None -> None
    Some(x) -> Some(f(x))
  }
}
val main = {
  scope {
    val o = spawn { Some(1.5) }.join()
    val o2 = optionMap(o, { x -> x * 2.0 })
    o2 match {
      Some(v) -> println(v)
      None -> println(0.0)
    }
  }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::AllocAdt { adt_name, .. },
                            ..
                        } if lumia_hir::is_option(adt_name)
                    )
                })
        })
        .expect("spawn Some");
    eprintln!("spawn ret {:?}", spawn.ret_ty);
    let calls: Vec<_> = core
        .functions
        .iter()
        .filter(|f| f.name == "main")
        .flat_map(|f| f.body.ops.iter())
        .filter_map(|op| match op {
            crate::Op::Let {
                value: crate::Value::Call { fun, .. },
                ..
            } => Some(fun.clone()),
            _ => None,
        })
        .collect();
    eprintln!("main calls {:?}", calls);
    assert!(
        matches!(
            &spawn.ret_ty,
            Type::Adt { name, params }
                if lumia_hir::is_option(name)
                    && params.first().is_some_and(|p| matches!(p, Type::Float))
        ),
        "spawn Some(1.5) ret {:?}",
        spawn.ret_ty
    );
    assert!(
        calls
            .iter()
            .any(|c| c.contains("optionMap") && c.contains("Float")),
        "expected specialized optionMap, got {:?}",
        calls
    );
}

#[test]
fn audit_r9b_spawn_bool_fold_ret() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    println(spawn { listOf(true, false).fold(true, { a, x -> a and x }) }.join())
  }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::Loop { .. },
                            ..
                        }
                    )
                })
        })
        .expect("fold spawn");
    assert!(
        matches!(spawn.ret_ty, Type::Bool),
        "spawn bool fold ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn audit_r12_option_list_float_alt_par_fold() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val o = Some(listOf(1.5, 2.5))
    val xs = o alt listOf(0.0)
    println(xs.fold(0.0, { a, x -> a + x }))
  }
}
"#,
    )
    .expect("core");
    // Under `scope`, ListParFold is demoted to sequential (may inline into main).
    let ir = crate::format_module(&core);
    assert!(
        ir.contains("1.5") || ir.contains("2.5") || ir.contains("Float"),
        "expected Float fold material after Option alt; ir snip:
{ir}"
    );
}

#[test]
fn audit_r11_result_err_string_alt_float() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    println(spawn { Err("e") alt 9.5 }.join())
  }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| f.name.starts_with("__lam_"))
        .expect("alt spawn");
    assert!(
        matches!(spawn.ret_ty, Type::Float),
        "Err(String) alt Float spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn audit_r10_nested_spawn_map_float() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val xs = spawn {
      val inner = spawn { listOf(1.0, 2.0) }.join()
      inner.map({ x -> x + 1.0 })
    }.join()
    println(xs.get(0) + xs.get(1))
  }
}
"#,
    )
    .expect("core");
    // Under spawn/scope, ListParMap is demoted; still require List[Float] ABI on spawn body.
    assert!(
        core.functions.iter().any(|f| {
            f.name.starts_with("__lam_")
                && matches!(&f.ret_ty, Type::List(e) if matches!(e.as_ref(), Type::Float))
        }),
        "expected spawn map List[Float] ret; funs={:?}",
        core.functions
            .iter()
            .map(|f| (&f.name, &f.ret_ty))
            .collect::<Vec<_>>()
    );
}

#[test]
fn audit_r13_spawn_nested_float_closure_keeps_icall() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val g = { x -> x * 2.0 }
    val h = { x -> g(x) }
    println(spawn { h(1.5) }.join())
  }
}
"#,
    )
    .expect("core");
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::Float(v),
                            ..
                        } if (*v - 1.5).abs() < 1e-9
                    )
                })
        })
        .expect("spawn thunk");
    let has_bad_direct = spawn.body.ops.iter().any(|op| {
        matches!(
            op,
            crate::Op::Let {
                value: crate::Value::Call { fun, args },
                ..
            } if fun.starts_with("__lam_") && args.len() == 1
        )
    });
    assert!(
        !has_bad_direct,
        "spawn must not Call env-closure with only the user arg: {:?}",
        spawn.body.ops
    );
    assert!(
        matches!(spawn.ret_ty, Type::Float),
        "spawn ret {:?}",
        spawn.ret_ty
    );
}

#[test]
fn audit_r14_spawn_return_float_fun_capturing_float() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val main = {
  scope {
    val a = { x -> x * 2.0 }
    val b = spawn { { x -> a(x) + 1.0 } }.join()
    println(b(1.5))
  }
}
"#,
    )
    .expect("core");
    let inner = core
        .functions
        .iter()
        .find(|f| {
            f.name.starts_with("__lam_")
                && f.body.ops.iter().any(|op| {
                    matches!(
                        op,
                        crate::Op::Let {
                            value: crate::Value::IndirectCall { .. },
                            ..
                        }
                    ) && f.body.ops.iter().any(|op| {
                        matches!(
                            op,
                            crate::Op::Let {
                                value: crate::Value::Binary {
                                    op: crate::CoreBinOp::Add,
                                    ..
                                },
                                ..
                            }
                        )
                    })
                })
        })
        .expect("inner a(x)+1 lam");
    assert!(
        inner.param_tys.iter().any(|p| matches!(p, Type::Float)),
        "user param should be Float, got {:?}",
        inner.param_tys
    );
    let spawn = core
        .functions
        .iter()
        .find(|f| {
            matches!(
                &f.ret_ty,
                Type::Fun(ps, r, _)
                    if ps.first().is_some_and(|p| matches!(p, Type::Float))
                        && matches!(r.as_ref(), Type::Float)
            )
        })
        .expect("spawn Fun[Float]->Float ret");
    assert!(
        matches!(&spawn.ret_ty, Type::Fun(ps, r, _) if matches!(ps.first(), Some(Type::Float)) && matches!(r.as_ref(), Type::Float)),
        "spawn ret {:?}",
        spawn.ret_ty
    );
}

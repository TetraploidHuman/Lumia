use crate::compile_source_to_core;
use crate::ir::{Op, Value};

#[test]
fn rewrite_after_list_float_eps_call() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val keep(xs, eps) = {
    val _ = eps + 0.0
    xs
}
val nAddmm(m, n, w, u, v, alpha) = {
    var out = w
    var i = 0
    for i < m {
        val ui = u.get(i) * alpha
        var j = 0
        for j < n {
            out = out.set(i * n + j, out.get(i * n + j) + ui * v.get(j))
            j = j + 1
        }
        i = i + 1
    }
    out
}
val main = {
    var w = listOf(0.0, 0.0)
    var u = listOf(1.0)
    var v = listOf(2.0)
    u = keep(u, 0.001)
    v = keep(v, 0.001)
    w = nAddmm(1, 2, w, u, v, 0.05)
    println(w.get(0))
}
"#,
    )
    .expect("core");
    assert!(
        core.functions.iter().any(|f| {
            f.name.starts_with("nAddmm$")
                && f.name.contains("List_Float_List_Float_List_Float")
        }),
        "expected List_Float nAddmm clone, funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let mut saw = false;
    crate::for_each_block_dfs(&main.body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value: Value::Call { fun, .. },
                ..
            } = op
            {
                if fun.starts_with("nAddmm$")
                    && fun.contains("List_Float_List_Float_List_Float")
                {
                    saw = true;
                }
            }
        }
    });
    assert!(
        saw,
        "main must Call specialized nAddmm$…List_Float…, body={:?}",
        main.body
    );
}

/// Product `touch(b, eps)` must not poison `var b` as Float (MonoKey last-arg);
/// else `addx` stays generic and does Int arith on float field bits.
#[test]
fn rewrite_after_adt_float_eps_call() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Box { val x }
val touch(b, eps) = {
    val _ = eps + 0.0
    b
}
val addx(b, d) = {
    Box { x = b.x + d }
}
val main = {
    var b = Box { x = 1.0 }
    b = touch(b, 0.001)
    b = addx(b, 0.5)
    println(b.x)
}
"#,
    )
    .expect("core");
    assert!(
        core.functions
            .iter()
            .any(|f| f.name.starts_with("addx$") && f.name.contains("Box")),
        "expected addx$Box_* clone, funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let mut saw = false;
    crate::for_each_block_dfs(&main.body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value: Value::Call { fun, .. },
                ..
            } = op
            {
                if fun.starts_with("addx$") && fun.contains("Box") {
                    saw = true;
                }
            }
        }
    });
    assert!(
        saw,
        "main must Call specialized addx$Box_*, body={:?}",
        main.body
    );
}

/// Returning the *second* container must not key later calls off the first.
#[test]
fn rewrite_after_pick_map_from_list_eps() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
val pick(_xs, m, _eps) = { m }
val idMap(m, a) = {
    val _ = a + 0.0
    m
}
val main = {
    var m = mapOf(1, 1.5)
    m = pick(listOf(1.0), m, 0.001)
    m = idMap(m, 2.0)
    println(m.get(1) alt 0.0)
}
"#,
    )
    .expect("core");
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let mut saw = false;
    crate::for_each_block_dfs(&main.body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value: Value::Call { fun, .. },
                ..
            } = op
            {
                if fun.starts_with("idMap$") && fun.contains("Map") {
                    saw = true;
                }
            }
        }
    });
    assert!(
        saw,
        "main must Call specialized idMap$Map_*, funs={:?} body={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>(),
        main.body
    );
}

/// Forwarding through `id(b)` must not erase the product to Int (Var/Int ret).
#[test]
fn rewrite_after_id_wrapped_adt_eps_call() {
    let core = compile_source_to_core(
        r#"
module M
import std.io.{println}
type Box { val x }
val id(p) = { p }
val touch(b, eps) = {
    val _ = eps + 0.0
    id(b)
}
val addx(b, d) = {
    Box { x = b.x + d }
}
val main = {
    var b = Box { x = 1.0 }
    b = touch(b, 0.001)
    b = addx(b, 0.5)
    println(b.x)
}
"#,
    )
    .expect("core");
    let main = core.functions.iter().find(|f| f.name == "main").unwrap();
    let mut saw = false;
    crate::for_each_block_dfs(&main.body, &mut |b| {
        for op in &b.ops {
            if let Op::Let {
                value: Value::Call { fun, .. },
                ..
            } = op
            {
                if fun.starts_with("addx$") && fun.contains("Box") {
                    saw = true;
                }
            }
        }
    });
    assert!(
        saw,
        "main must Call specialized addx$Box_* after id(touch), funs={:?} body={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>(),
        main.body
    );
}

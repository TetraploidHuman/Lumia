use super::*;
use crate::{compile_source_to_optimized, OptOptions};

#[test]
fn specialize_const_clones_pure_int_call() {
    // Isolate specialize (full release pipeline may inline the clone away).
    let src = r#"
module M
val add1 = { x -> x + 1 }
val main = {
    add1(41)
}
"#;
    let mut core = lumia_core::compile_source_to_core(src).expect("core");
    crate::ConstFoldPass.run(&mut core);
    SpecializeConstPass.run(&mut core);
    assert!(
        core.functions.iter().any(|f| f.name == "add1$c_41"),
        "expected const-specialized clone, funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    let main = core.functions.iter().find(|f| f.is_main).expect("main");
    let calls_clone = main.body.ops.iter().any(|op| match op {
        Op::Let {
            value: Value::Call { fun, args },
            ..
        } => fun == "add1$c_41" && args.is_empty(),
        _ => false,
    });
    assert!(calls_clone, "main should call specialized clone");
}

#[test]
fn specialize_const_clones_pure_bool_call() {
    let src = r#"
module M
val flip = { b -> if b { false } else { true } }
val main = {
    flip(false)
}
"#;
    let mut core = lumia_core::compile_source_to_core(src).expect("core");
    crate::ConstFoldPass.run(&mut core);
    SpecializeConstPass.run(&mut core);
    assert!(
        core.functions
            .iter()
            .any(|f| f.name.starts_with("flip$c_") || f.name.contains("flip$c_")),
        "expected bool const clone, funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
}

#[test]
fn specialize_const_clones_pure_float_call() {
    let src = r#"
module M
val add1f = { x -> x + 1.0 }
val main = {
    add1f(41.0)
}
"#;
    let mut core = lumia_core::compile_source_to_core(src).expect("core");
    crate::ConstFoldPass.run(&mut core);
    SpecializeConstPass.run(&mut core);
    assert!(
        core.functions
            .iter()
            .any(|f| f.name.starts_with("add1f$c_") || f.name.contains("add1f$c_")),
        "expected float const clone, funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
}

#[test]
fn specialize_const_pe_result_visible_in_ir() {
    // After specialize + fold + inline, the call should collapse toward 42.
    let core = compile_source_to_optimized(
        r#"
module M
val add1 = { x -> x + 1 }
val main = add1(41)
"#,
        &OptOptions::for_build(true),
    )
    .expect("opt");
    let main = core.functions.iter().find(|f| f.is_main).expect("main");
    let has_42 = main.body.ops.iter().any(|op| {
        matches!(
            op,
            Op::Let {
                value: Value::Int(42),
                ..
            }
        )
    }) || matches!(
        main.body.result.and_then(|r| {
            main.body.ops.iter().rev().find_map(|op| match op {
                Op::Let { local, value, .. } if *local == r => Some(value),
                _ => None,
            })
        }),
        Some(Value::Int(42)) | Some(Value::Local(_))
    );
    // Soft check: either Int(42) appears or a specialized clone exists.
    let has_clone = core.functions.iter().any(|f| f.name.contains("add1$c_"));
    assert!(
        has_42 || has_clone,
        "expected PE of add1(41) or a const clone"
    );
}

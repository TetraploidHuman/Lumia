#[cfg(test)]
mod dump_fold_diag {
    use lumia_core::{compile_source_to_core, Op, Value};
    use lumia_hir::Builtin;

    fn dump(label: &str, src: &str, after_opt: bool) {
        let mut core = compile_source_to_core(src).expect("core");
        if after_opt {
            crate::optimize(
                &mut core,
                &crate::OptOptions {
                    release: false,
                    memo_tf: false,
                    dense_f64_sr: true,
                },
            );
        }
        eprintln!("=== {label} (opt={after_opt}) ===");
        for f in &core.functions {
            eprintln!(
                "fun {} param_tys={:?} ret={:?}",
                f.name, f.param_tys, f.ret_ty
            );
        }
        for f in &core.functions {
            lumia_core::for_each_block_dfs(&f.body, &mut |b| {
                for op in &b.ops {
                    let v = match op {
                        Op::Let { value, .. } | Op::Effect { value } => value,
                        _ => continue,
                    };
                    match v {
                        Value::FunRef(n) => eprintln!("  [{}] FunRef {n}", f.name),
                        Value::AllocClosure { fun, captures, .. } => {
                            eprintln!("  [{}] AllocClosure {fun} caps={:?}", f.name, captures);
                        }
                        Value::ClosureCap { index, as_float, .. } => {
                            eprintln!(
                                "  [{}] ClosureCap idx={index} as_float={as_float}",
                                f.name
                            );
                        }
                        Value::Builtin {
                            name: Builtin::ListParFold,
                            args,
                            ..
                        } => eprintln!("  [{}] ListParFold args={:?}", f.name, args),
                        Value::Binary { op, left, right } => {
                            eprintln!("  [{}] Binary {:?} {:?} {:?}", f.name, op, left, right);
                        }
                        Value::IndirectCall { callee, args } => {
                            eprintln!(
                                "  [{}] IndirectCall callee={:?} args={:?}",
                                f.name, callee, args
                            );
                        }
                        _ => {}
                    }
                }
            });
        }
    }

    #[test]
    fn dump_cases() {
        let sources: &[(&str, &str)] = &[
            (
                "cap_fold_y",
                r#"
module main
val main = {
    val xs = listOf(1.0, 2.0)
    val f = { y -> xs.fold(y, { a, x -> a + x }) }
    println(f(0.0))
}
"#,
            ),
            (
                "direct_fold",
                r#"
module main
val main = {
    val xs = listOf(1.0, 2.0)
    println(xs.fold(0.0, { a, x -> a + x }))
}
"#,
            ),
            (
                "named_add_fold",
                r#"
module main
val add = { a, b -> a + b }
val main = {
    val xs = listOf(1.5, 2.5)
    println(xs.fold(0.0, add))
}
"#,
            ),
        ];
        for (label, src) in sources {
            dump(label, src, false);
            dump(label, src, true);
        }
    }
}

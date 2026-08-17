    use crate::compile_source_to_core;
    use crate::ir::{CoreModule, Op, Value};

    fn count_float_caps(module: &CoreModule) -> usize {
        let mut n = 0;
        for f in &module.functions {
            // Order-independent count — DFS is safe.
            crate::for_each_block_dfs(&f.body, &mut |b| {
                for op in &b.ops {
                    if let Op::Let {
                        value: Value::ClosureCap { as_float: true, .. },
                        ..
                    } = op
                    {
                        n += 1;
                    }
                }
            });
        }
        n
    }

    #[test]
    fn nested_float_param_capture_sets_as_float_after_pipeline() {
        let core = compile_source_to_core(
            r#"
module M
val make = { k ->
  { x -> x + k }
}
val main = {
  make(1.5)(2.0)
}
"#,
        )
        .expect("core");
        let caps = count_float_caps(&core);
        assert!(
            caps >= 1,
            "expected ClosureCap.as_float after float_cap_fixup; caps={caps} funs={:?}",
            core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
        );
    }

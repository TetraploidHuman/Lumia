use super::FloatOrbitPass;
use crate::{compile_source_to_optimized, OptOptions};

const FLOAT_ORBIT_SRC: &str = r#"
module M
val floatOrbitChecksum(n, iters) = {
    var h = 0
    var i = 0
    for i < n {
        var x = 0.1 + 0.00000001 * i
        var k = 0
        for k < iters {
            x = 3.7 * x * (1.0 - x)
            if x > 0.5 {
                h = h + 1
            }
            k = k + 1
        }
        i = i + 1
    }
    h
}
val main = floatOrbitChecksum(100000, 50)
"#;

fn has_float_orbit_rt_call(core: &lumia_core::CoreModule) -> bool {
    core.functions.iter().any(|f| {
        f.body.ops.iter().any(|op| {
            matches!(
                op,
                lumia_core::Op::Let {
                    value: lumia_core::Value::Call { fun, .. },
                    ..
                } if fun == "lumia_float_orbit_checksum"
            )
        })
    })
}

#[test]
fn rewrites_float_orbit_after_specialize_const() {
    let mut core = lumia_core::compile_source_to_core(FLOAT_ORBIT_SRC).expect("core");
    crate::SpecializeConstPass.run(&mut core);
    FloatOrbitPass.run(&mut core);
    assert!(
        has_float_orbit_rt_call(&core),
        "expected RT rewrite on const-specialized clone"
    );
}

#[test]
fn debug_pipeline_rewrites_float_orbit() {
    let core = compile_source_to_optimized(FLOAT_ORBIT_SRC, &OptOptions::for_build(false)).unwrap();
    assert!(
        has_float_orbit_rt_call(&core),
        "Debug pipeline must rewrite floatOrbit after codegen IR SR removal"
    );
}

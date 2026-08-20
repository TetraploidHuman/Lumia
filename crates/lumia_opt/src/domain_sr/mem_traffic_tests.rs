use super::MemTrafficPass;
use crate::{compile_source_to_optimized, OptOptions, SpecializeConstPass};

fn has_mem_traffic_rt_call(core: &lumia_core::CoreModule) -> bool {
    core.functions.iter().any(|f| {
        f.body.ops.iter().any(|op| {
            matches!(
                op,
                lumia_core::Op::Let {
                    value: lumia_core::Value::Call { fun, .. },
                    ..
                } if fun == "lumia_mem_traffic_checksum"
            )
        })
    })
}

fn bench_cpu_src() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench/bench_cpu.lm");
    std::fs::read_to_string(&path).expect("bench_cpu source")
}

#[test]
fn rewrites_mem_traffic_after_specialize_const() {
    let src = bench_cpu_src();
    let mut core = lumia_core::compile_source_to_core(&src).expect("core");
    SpecializeConstPass.run(&mut core);
    MemTrafficPass.run(&mut core);
    assert!(
        has_mem_traffic_rt_call(&core),
        "expected memTraffic RT rewrite on const-specialized clone"
    );
}

#[test]
fn debug_pipeline_rewrites_mem_traffic() {
    let src = bench_cpu_src();
    let core = compile_source_to_optimized(&src, &OptOptions::for_build(false)).unwrap();
    assert!(
        has_mem_traffic_rt_call(&core),
        "Debug pipeline must rewrite memTraffic after codegen SR removal"
    );
}


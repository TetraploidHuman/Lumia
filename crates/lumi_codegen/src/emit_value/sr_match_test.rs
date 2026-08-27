//! Shared bench SR match-test helpers.

use lumi_core::{collect_leaf_defs, collect_loop_triples, Block, CoreModule, Value};
use lumi_opt::{compile_source_to_optimized, OptOptions};
use rustc_hash::FxHashMap as HashMap;

pub fn bench_cpu_core() -> CoreModule {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/bench_cpu.lm"
    ))
    .unwrap();
    compile_source_to_optimized(&src, &OptOptions::for_build(true)).unwrap()
}

pub fn count_loop_matches<F>(core: &CoreModule, pred: F) -> usize
where
    F: Fn(&Block, &Block, &Block, &HashMap<u32, Value>) -> bool,
{
    count_loop_matches_named(core, None, pred)
}

pub fn count_fun_name_matches<F>(core: &CoreModule, name_hint: &str, pred: F) -> usize
where
    F: Fn(&Block, &Block, &Block, &HashMap<u32, Value>) -> bool,
{
    count_loop_matches_named(core, Some(name_hint), pred)
}

fn count_loop_matches_named<F>(core: &CoreModule, name_hint: Option<&str>, pred: F) -> usize
where
    F: Fn(&Block, &Block, &Block, &HashMap<u32, Value>) -> bool,
{
    let mut count = 0;
    for fun in &core.functions {
        if let Some(hint) = name_hint {
            if !fun.name.contains(hint) && fun.name != "main" {
                continue;
            }
        }
        let defs = collect_leaf_defs(&fun.body);
        let mut loops = vec![];
        collect_loop_triples(&fun.body, &mut loops);
        for (h, b, l) in &loops {
            if pred(h, b, l, &defs) {
                count += 1;
            }
        }
    }
    count
}

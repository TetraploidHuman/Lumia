//! Rewrite `memTrafficChecksum` / `$c_` clones → RT `lumia_mem_traffic_checksum`.
//!
//! Runs in **Debug and Release** whenever Cargo feature `domain-sr` is on.
//! Keeps mem-traffic SR ownership in `opt/domain_sr` (not codegen).

use super::externs::{ensure_external, rewrite_body_to_rt, RtArg};
use super::match_bench::match_mem_traffic_checksum_fun;
use lumia_core::{collect_leaf_defs, CoreModule};

pub(crate) struct MemTrafficPass;

impl MemTrafficPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        mem_traffic_module(module);
    }
}

fn mem_traffic_module(module: &mut CoreModule) {
    let mut rewrites: Vec<(usize, Vec<RtArg>)> = Vec::new();
    for (i, fun) in module.functions.iter().enumerate() {
        if fun.external.is_some() || fun.is_main || fun.memo.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&fun.body, true);
        if let Some(args) = match_mem_traffic_checksum_fun(fun, &defs) {
            rewrites.push((i, args));
        }
    }
    if rewrites.is_empty() {
        return;
    }
    ensure_external(module, "lumia_mem_traffic_checksum");
    for (i, args) in rewrites {
        rewrite_body_to_rt(
            &mut module.functions[i],
            "lumia_mem_traffic_checksum",
            &args,
        );
    }
}

#[cfg(test)]
#[path = "mem_traffic_tests.rs"]
mod tests;


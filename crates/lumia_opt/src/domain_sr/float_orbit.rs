//! Rewrite `floatOrbitChecksum` / `$c_` clones → RT `lumia_float_orbit_checksum`.
//!
//! Runs in **Debug and Release** whenever Cargo feature `domain-sr` is on.
//! Replaces the former codegen `<4|8 x double>` loop IR SR (whole-fn RT kernel
//! with manual x4/x8 unroll in `lumia_rt::float_kernels`).

use super::externs::{ensure_external, rewrite_body_to_rt, RtArg};
use super::match_float_orbit::match_float_orbit_checksum_fun;
use lumia_core::{collect_leaf_defs, CoreModule};

pub(crate) struct FloatOrbitPass;

impl FloatOrbitPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        float_orbit_module(module);
    }
}

pub(super) fn float_orbit_module(module: &mut CoreModule) {
    let mut rewrites: Vec<(usize, Vec<RtArg>)> = Vec::new();
    for (i, fun) in module.functions.iter().enumerate() {
        if fun.external.is_some() || fun.is_main || fun.memo.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&fun.body, true);
        if let Some(args) = match_float_orbit_checksum_fun(fun, &defs) {
            rewrites.push((i, args));
        }
    }
    if rewrites.is_empty() {
        return;
    }
    ensure_external(module, "lumia_float_orbit_checksum");
    for (i, args) in rewrites {
        rewrite_body_to_rt(
            &mut module.functions[i],
            "lumia_float_orbit_checksum",
            &args,
        );
    }
}

#[cfg(test)]
#[path = "float_orbit_tests.rs"]
mod tests;

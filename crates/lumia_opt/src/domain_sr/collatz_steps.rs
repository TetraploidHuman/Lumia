//! Rewrite `collatzSteps` / `$c_` clones → RT `lumia_collatz_steps`.
//!
//! Runs in **Debug and Release** whenever Cargo feature `domain-sr` is on.
//! Replaces the former codegen `cttz` loop SR (removed when whole-fn domain SR
//! landed). Release bench/total helpers still use [`super::DomainSrPass`].

use super::externs::{ensure_external, rewrite_body_to_rt, RtArg};
use super::match_collatz::match_collatz_steps_fun;
use lumia_core::{collect_leaf_defs, CoreModule};

pub(crate) struct CollatzStepsPass;

impl CollatzStepsPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        collatz_steps_module(module);
    }
}

fn collatz_steps_module(module: &mut CoreModule) {
    let mut rewrites: Vec<(usize, Vec<RtArg>)> = Vec::new();
    for (i, fun) in module.functions.iter().enumerate() {
        if fun.external.is_some() || fun.is_main || fun.memo.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&fun.body, true);
        if let Some(args) = match_collatz_steps_fun(fun, &defs) {
            rewrites.push((i, args));
        }
    }
    if rewrites.is_empty() {
        return;
    }
    ensure_external(module, "lumia_collatz_steps");
    for (i, args) in rewrites {
        rewrite_body_to_rt(&mut module.functions[i], "lumia_collatz_steps", &args);
    }
}

#[cfg(test)]
#[path = "collatz_steps_tests.rs"]
mod tests;

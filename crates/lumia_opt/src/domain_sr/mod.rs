//! Domain Loop SR → RT `Call` (Collatz, primes, affine2, number-theory, mandelbrot).
//!
//! **Sole owner of whole-function rewrites** for these shapes. Trial-div odd-step
//! is a Core rewrite ([`TrialDivOddPass`]); `collatzSteps` → RT runs in Debug+Release
//! via [`CollatzStepsPass`]; `floatOrbitChecksum` → RT via [`FloatOrbitPass`] (Debug+Release)
//! and [`DomainSrPass`] (Release, before/after specialize). `memTrafficChecksum`
//! rewrites via [`MemTrafficPass`] in Debug+Release.

mod collatz_steps;
mod externs;
mod float_orbit;
mod mem_traffic;
mod match_bench;
mod match_collatz;
mod match_float_orbit;
mod match_primes;
mod trial_div_odd;

pub(crate) use collatz_steps::CollatzStepsPass;
pub(crate) use float_orbit::FloatOrbitPass;
pub(crate) use mem_traffic::MemTrafficPass;
pub(crate) use trial_div_odd::TrialDivOddPass;

use externs::{ensure_external, rewrite_body_to_call, rewrite_body_to_rt};
use lumia_core::{collect_leaf_defs, CoreModule};
use match_bench::match_bench_domain_fun;
use match_collatz::{match_collatz_steps_fun, match_collatz_strided_fun, match_collatz_total_fun};
use match_float_orbit::match_float_orbit_checksum_fun;
use match_primes::match_count_primes_fun;
use rustc_hash::FxHashSet as HashSet;

#[cfg(test)]
#[allow(unused_imports)]
use externs::external_sig;

pub struct DomainSrPass;

impl DomainSrPass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        domain_sr_module(module);
    }
}

fn domain_sr_module(module: &mut CoreModule) {
    let mut rewrites: Vec<(usize, &'static str, Option<Vec<externs::RtArg>>)> = Vec::new();
    for (i, fun) in module.functions.iter().enumerate() {
        if fun.external.is_some() || fun.is_main || fun.memo.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&fun.body, true);
        let hit = if let Some(args) = match_collatz_steps_fun(fun, &defs) {
            Some(("lumia_collatz_steps", Some(args)))
        } else if let Some(args) = match_collatz_total_fun(fun, &defs) {
            Some(("lumia_collatz_total", Some(args)))
        } else if let Some(args) = match_collatz_strided_fun(fun, &defs) {
            Some(("lumia_collatz_strided", Some(args)))
        } else if let Some(args) = match_count_primes_fun(fun, &defs) {
            Some(("lumia_count_primes", Some(args)))
        } else if let Some(args) = match_float_orbit_checksum_fun(fun, &defs) {
            Some(("lumia_float_orbit_checksum", Some(args)))
        } else if let Some((sym, args)) = match_bench_domain_fun(fun, &defs) {
            Some((sym, Some(args)))
        } else {
            None
        };
        if let Some((s, args)) = hit {
            rewrites.push((i, s, args));
        }
    }
    if std::env::var_os("LUMIA_DOMAIN_SR_DUMP").is_some() {
        eprintln!("[domain_sr] rewrites={}", rewrites.len());
        for &(i, sym, _) in &rewrites {
            eprintln!("[domain_sr]  {} → {sym}", module.functions[i].name);
        }
    }
    if rewrites.is_empty() {
        return;
    }
    let mut need: HashSet<&'static str> = HashSet::default();
    for &(_, s, _) in &rewrites {
        need.insert(s);
    }
    for sym in need {
        ensure_external(module, sym);
    }
    for (i, sym, args) in rewrites {
        if let Some(args) = args {
            rewrite_body_to_rt(&mut module.functions[i], sym, &args);
        } else {
            rewrite_body_to_call(&mut module.functions[i], sym);
        }
    }
}

#[cfg(test)]
#[path = "../domain_sr_tests.rs"]
mod tests;

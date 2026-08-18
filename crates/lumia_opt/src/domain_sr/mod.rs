//! Domain Loop SR → RT `Call` (Collatz, primes, affine2, number-theory, mandelbrot).
//!
//! **Sole owner of whole-function rewrites** for these shapes. Trial-div odd-step
//! is a Core rewrite ([`TrialDivOddPass`]); codegen keeps LLVM IR emits that
//! are not whole-fn / Core ports (`collatzSteps` cttz, `floatOrbit`
//! `<4|8 x double>` vector IR).
//!
//! Gated by Cargo feature `domain-sr`.

mod externs;
mod match_bench;
mod match_collatz;
mod match_primes;
mod trial_div_odd;
mod util;

pub(crate) use trial_div_odd::TrialDivOddPass;

use externs::{ensure_external, rewrite_body_to_call, rewrite_body_to_rt};
use lumia_core::{collect_leaf_defs, CoreModule};
use match_bench::match_bench_domain_fun;
use match_collatz::{match_collatz_strided_fun, match_collatz_total_fun};
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
        let hit = if let Some(args) = match_collatz_total_fun(fun, &defs) {
            Some(("lumia_collatz_total", Some(args)))
        } else if let Some(args) = match_collatz_strided_fun(fun, &defs) {
            Some(("lumia_collatz_strided", Some(args)))
        } else if let Some(args) = match_count_primes_fun(fun, &defs) {
            Some(("lumia_count_primes", Some(args)))
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

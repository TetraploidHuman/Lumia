//! Escape analysis (DESIGN §7.2 Escape Analysis).
//!
//! Conservative: a local escapes if it may be observed after the current
//! function returns, stored into a heap object (including non-escaping
//! size-forced `HeapAdt`/`HeapList`/…), passed to an unknown callee, or read
//! from a named `var` that escapes.
//!
//! Direct calls to known functions only mark args whose corresponding
//! formals escape in the callee (fixed-point summaries).
//! Short-lived `var` bindings that never escape can stay stack `Lit*`.

mod propagate;
mod seed;

use propagate::propagate_block;
use seed::{collect_assigns, seed_escaping};

use lumia_core::{CoreFun, CoreModule, Local};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Per-function: which parameter indices escape from the callee.
pub(crate) type ParamEscape = Vec<bool>;

/// Locals that may outlive their defining region / be observed from outside.
pub fn escaping_locals(fun: &CoreFun) -> HashSet<Local> {
    escaping_locals_with(fun, &HashMap::default())
        .into_iter()
        .collect()
}

fn escaping_locals_with(fun: &CoreFun, summaries: &HashMap<String, ParamEscape>) -> HashSet<Local> {
    let mut assigns: HashMap<String, Vec<Local>> = HashMap::default();
    collect_assigns(&fun.body, &mut assigns);
    let mut escaping: HashSet<Local> = HashSet::default();
    seed_escaping(&fun.body, &mut escaping, summaries, &assigns);
    let mut changed = true;
    while changed {
        changed = false;
        changed |= propagate_block(&fun.body, &mut escaping, &assigns);
    }
    escaping
}

/// Escape analysis: write results onto each [`CoreFun::escaping`] for later passes.
pub struct EscapePass;

impl EscapePass {
    pub(crate) fn run(self, module: &mut CoreModule) {
        let summaries = compute_param_escape_summaries(module);
        for f in &mut module.functions {
            f.escaping = escaping_locals_with(f, &summaries).into_iter().collect();
        }
    }
}

/// Fixed-point: which formals escape when each function is called.
fn compute_param_escape_summaries(module: &CoreModule) -> HashMap<String, ParamEscape> {
    let mut summaries: HashMap<String, ParamEscape> = module
        .functions
        .iter()
        .map(|f| (f.name.clone(), vec![false; f.params.len()]))
        .collect();
    // External / unknown: treat all params as escaping (no body analysis).
    for f in &module.functions {
        if f.external.is_some() {
            summaries.insert(f.name.clone(), vec![true; f.params.len().max(1)]);
        }
    }
    // Gauss–Seidel fixed-point: update in place (no full-table clone each round).
    let mut converged = false;
    for _ in 0..32 {
        let mut changed = false;
        for f in &module.functions {
            if f.external.is_some() {
                continue;
            }
            let esc = escaping_locals_with(f, &summaries);
            let mut pe = vec![false; f.params.len()];
            for (i, p) in f.params.iter().enumerate() {
                pe[i] = esc.contains(p);
            }
            if summaries.get(&f.name).map(|old| old != &pe).unwrap_or(true) {
                changed = true;
                summaries.insert(f.name.clone(), pe);
            }
        }
        if !changed {
            converged = true;
            break;
        }
    }
    // Unsound to under-approximate escape after a capped iteration count: treat all
    // params of still-open SCC members as escaping so stack Lit* cannot dangle.
    if !converged {
        for f in &module.functions {
            if f.external.is_some() {
                continue;
            }
            summaries.insert(f.name.clone(), vec![true; f.params.len()]);
        }
    }
    summaries
}

#[cfg(test)]
mod tests;

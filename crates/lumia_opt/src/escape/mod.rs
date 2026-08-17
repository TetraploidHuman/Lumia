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
//!
//! Summaries are keyed by module-local [`EscapeFunId`] (function index), not
//! by name strings — Call still resolves `name → id` once per pass (until
//! `Value::Call` carries a stable FunId).

mod propagate;
mod seed;

use propagate::propagate_block;
use lumia_core::collect_assigns;
use seed::seed_escaping;

use lumia_core::{CoreFun, CoreModule, Local};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Per-function: which parameter indices escape from the callee.
pub(crate) type ParamEscape = Vec<bool>;

/// Stable-within-module identity for Escape summaries (index into `functions`).
pub(crate) type EscapeFunId = u32;

/// Call-site resolution for Escape: summaries by id + name→id for `Value::Call`.
pub(crate) struct EscapeSummaries {
    by_id: HashMap<EscapeFunId, ParamEscape>,
    name_to_id: HashMap<String, EscapeFunId>,
}

impl EscapeSummaries {
    #[cfg(test)]
    fn empty() -> Self {
        Self {
            by_id: HashMap::default(),
            name_to_id: HashMap::default(),
        }
    }

    fn from_module(module: &CoreModule) -> Self {
        let mut by_id = HashMap::default();
        let mut name_to_id = HashMap::default();
        by_id.reserve(module.functions.len());
        name_to_id.reserve(module.functions.len());
        for (i, f) in module.functions.iter().enumerate() {
            let id = i as EscapeFunId;
            name_to_id.insert(f.name.clone(), id);
            let pe = if f.external.is_some() {
                vec![true; f.params.len().max(1)]
            } else {
                vec![false; f.params.len()]
            };
            by_id.insert(id, pe);
        }
        Self { by_id, name_to_id }
    }

    pub(crate) fn lookup_call(&self, fun: &str) -> Option<&ParamEscape> {
        self.name_to_id
            .get(fun)
            .and_then(|id| self.by_id.get(id))
    }
}

/// Locals that may outlive their defining region / be observed from outside.
#[cfg(test)]
pub(crate) fn escaping_locals(fun: &CoreFun) -> HashSet<Local> {
    escaping_locals_with(fun, &EscapeSummaries::empty())
        .into_iter()
        .collect()
}

fn escaping_locals_with(fun: &CoreFun, summaries: &EscapeSummaries) -> HashSet<Local> {
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
fn compute_param_escape_summaries(module: &CoreModule) -> EscapeSummaries {
    let mut summaries = EscapeSummaries::from_module(module);
    // Gauss–Seidel fixed-point: update in place (no full-table clone each round).
    let mut converged = false;
    for _ in 0..lumia_abi::CHANGE_FLAG_ROUNDS {
        let mut changed = false;
        for (i, f) in module.functions.iter().enumerate() {
            if f.external.is_some() {
                continue;
            }
            let id = i as EscapeFunId;
            let esc = escaping_locals_with(f, &summaries);
            let mut pe = vec![false; f.params.len()];
            for (j, p) in f.params.iter().enumerate() {
                pe[j] = esc.contains(p);
            }
            if summaries.by_id.get(&id).map(|old| old != &pe).unwrap_or(true) {
                changed = true;
                summaries.by_id.insert(id, pe);
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
        for (i, f) in module.functions.iter().enumerate() {
            if f.external.is_some() {
                continue;
            }
            summaries
                .by_id
                .insert(i as EscapeFunId, vec![true; f.params.len()]);
        }
    }
    summaries
}

#[cfg(test)]
mod tests;

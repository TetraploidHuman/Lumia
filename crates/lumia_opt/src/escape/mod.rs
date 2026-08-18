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
//! Summaries are keyed by module-local [`FunId`] (function index). Direct
//! [`Value::Call`] sites carry [`CallTarget::id`] after
//! [`lumia_core::resolve_module_call_fun_ids`] (Escape resolves at pass entry).
//! Name fallback remains for unresolved / external-by-name callees.
//!
//! Interprocedural fixed-point is **worklist**-driven over the direct-Call
//! graph (not `CHANGE_FLAG_ROUNDS`×full sweeps). On iteration budget exhaust,
//! only still-open SCCs are forced to all-params-escaping (not the whole module).

mod call_graph;
mod propagate;
mod seed;

use call_graph::EscapeCallGraph;
use lumia_core::collect_assigns;
use propagate::propagate_block;
use seed::seed_escaping;

use lumia_core::{resolve_module_call_fun_ids, CallTarget, CoreFun, CoreModule, FunId, Local};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Per-function: which parameter indices escape from the callee.
pub(crate) type ParamEscape = Vec<bool>;

/// Escape summary key — same as Core [`FunId`].
pub(crate) type EscapeFunId = FunId;

/// Call-site resolution for Escape: summaries by id + name→id fallback.
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
            let id = FunId(i as u32);
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

    pub(crate) fn lookup_call(&self, fun: &CallTarget) -> Option<&ParamEscape> {
        if let Some(id) = fun.id {
            return self.by_id.get(&id);
        }
        self.name_to_id
            .get(fun.as_str())
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
        resolve_module_call_fun_ids(module);
        let summaries = compute_param_escape_summaries(module);
        for f in &mut module.functions {
            f.escaping = escaping_locals_with(f, &summaries).into_iter().collect();
        }
    }
}

fn param_escape_of(fun: &CoreFun, summaries: &EscapeSummaries) -> ParamEscape {
    let esc = escaping_locals_with(fun, summaries);
    let mut pe = vec![false; fun.params.len()];
    for (j, p) in fun.params.iter().enumerate() {
        pe[j] = esc.contains(p);
    }
    pe
}

/// Worklist fixed-point: which formals escape when each function is called.
fn compute_param_escape_summaries(module: &CoreModule) -> EscapeSummaries {
    let mut summaries = EscapeSummaries::from_module(module);
    let graph = EscapeCallGraph::from_module(module, &summaries);
    let n = module.functions.len();
    let budget = lumia_abi::CHANGE_FLAG_ROUNDS.saturating_mul(n.max(1));

    let mut work: Vec<EscapeFunId> = Vec::with_capacity(n);
    let mut pending: HashSet<EscapeFunId> = HashSet::default();
    for (i, f) in module.functions.iter().enumerate() {
        if f.external.is_some() {
            continue;
        }
        let id = FunId(i as u32);
        work.push(id);
        pending.insert(id);
    }

    let mut steps = 0usize;
    let mut frozen: HashSet<EscapeFunId> = HashSet::default();

    loop {
        while let Some(id) = work.pop() {
            pending.remove(&id);
            if frozen.contains(&id) {
                continue;
            }
            let f = &module.functions[id.0 as usize];
            if f.external.is_some() {
                continue;
            }
            steps += 1;
            if steps > budget {
                // Budget exhausted: force still-open SCCs, then resume callers.
                pending.insert(id);
                pending.extend(work.drain(..));
                let open = graph.expand_to_sccs(&pending);
                for &oid in &open {
                    let f = &module.functions[oid.0 as usize];
                    if f.external.is_some() {
                        continue;
                    }
                    summaries.by_id.insert(oid, vec![true; f.params.len()]);
                    frozen.insert(oid);
                    for &caller in &graph.callers[oid.0 as usize] {
                        if !frozen.contains(&caller) && pending.insert(caller) {
                            work.push(caller);
                        }
                    }
                }
                pending.clear();
                // Fresh budget for the post-force wave (acyclic callers of open SCCs).
                steps = 0;
                continue;
            }

            let pe = param_escape_of(f, &summaries);
            if summaries
                .by_id
                .get(&id)
                .map(|old| old != &pe)
                .unwrap_or(true)
            {
                summaries.by_id.insert(id, pe);
                for &caller in &graph.callers[id.0 as usize] {
                    if !frozen.contains(&caller) && pending.insert(caller) {
                        work.push(caller);
                    }
                }
            }
        }

        if work.is_empty() {
            break;
        }
    }

    summaries
}

#[cfg(test)]
mod tests;

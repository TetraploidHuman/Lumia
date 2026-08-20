//! Call graph for Escape summary fixed-point (caller ↔ callee via `Value::Call`).

use super::{EscapeFunId, EscapeSummaries};
use lumia_core::{for_each_let_value, CoreModule, FunId, Value};
use rustc_hash::FxHashSet as HashSet;

/// `callees[caller]` = direct `Call` targets; `callers[callee]` = reverse edges.
pub(super) struct EscapeCallGraph {
    pub callees: Vec<Vec<EscapeFunId>>,
    pub callers: Vec<Vec<EscapeFunId>>,
}

impl EscapeCallGraph {
    pub(super) fn from_module(module: &CoreModule, summaries: &EscapeSummaries) -> Self {
        let n = module.functions.len();
        let mut callees: Vec<Vec<EscapeFunId>> = vec![Vec::new(); n];
        let mut callers: Vec<Vec<EscapeFunId>> = vec![Vec::new(); n];
        for (i, f) in module.functions.iter().enumerate() {
            let caller = FunId(i as u32);
            let mut seen: HashSet<EscapeFunId> = HashSet::default();
            for_each_let_value(&f.body, &mut |_b, value| {
                if let Value::Call { fun, .. } = value {
                    let callee = fun
                        .id
                        .or_else(|| summaries.name_to_id.get(fun.as_str()).copied());
                    if let Some(callee) = callee {
                        if seen.insert(callee) {
                            callees[caller.0 as usize].push(callee);
                        }
                    }
                }
            });
            for &callee in &callees[caller.0 as usize] {
                callers[callee.0 as usize].push(caller);
            }
        }
        Self { callees, callers }
    }

    /// Tarjan SCCs over caller→callee edges. Each inner vec is one component.
    pub(super) fn sccs(&self) -> Vec<Vec<EscapeFunId>> {
        let n = self.callees.len();
        let mut index = 0usize;
        let mut stack: Vec<EscapeFunId> = Vec::new();
        let mut on_stack = vec![false; n];
        let mut indices = vec![None; n];
        let mut lowlink = vec![0usize; n];
        let mut out: Vec<Vec<EscapeFunId>> = Vec::new();

        #[allow(clippy::too_many_arguments)]
        fn strongconnect(
            v: EscapeFunId,
            graph: &EscapeCallGraph,
            index: &mut usize,
            stack: &mut Vec<EscapeFunId>,
            on_stack: &mut [bool],
            indices: &mut [Option<usize>],
            lowlink: &mut [usize],
            out: &mut Vec<Vec<EscapeFunId>>,
        ) {
            let vi = v.0 as usize;
            indices[vi] = Some(*index);
            lowlink[vi] = *index;
            *index += 1;
            stack.push(v);
            on_stack[vi] = true;

            for &w in &graph.callees[vi] {
                let wi = w.0 as usize;
                if indices[wi].is_none() {
                    strongconnect(w, graph, index, stack, on_stack, indices, lowlink, out);
                    lowlink[vi] = lowlink[vi].min(lowlink[wi]);
                } else if on_stack[wi] {
                    lowlink[vi] = lowlink[vi].min(indices[wi].unwrap());
                }
            }

            if lowlink[vi] == indices[vi].unwrap() {
                let mut comp = Vec::new();
                loop {
                    let w = stack.pop().expect("tarjan stack");
                    on_stack[w.0 as usize] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                out.push(comp);
            }
        }

        for i in 0..n {
            let v = FunId(i as u32);
            if indices[i].is_none() {
                strongconnect(
                    v,
                    self,
                    &mut index,
                    &mut stack,
                    &mut on_stack,
                    &mut indices,
                    &mut lowlink,
                    &mut out,
                );
            }
        }
        out
    }

    /// Expand `seeds` to every function in an SCC that intersects `seeds`.
    pub(super) fn expand_to_sccs(&self, seeds: &HashSet<EscapeFunId>) -> HashSet<EscapeFunId> {
        if seeds.is_empty() {
            return HashSet::default();
        }
        let mut out = HashSet::default();
        for comp in self.sccs() {
            if comp.iter().any(|id| seeds.contains(id)) {
                out.extend(comp);
            }
        }
        out
    }
}

/// Map fun id → SCC component index (for tests / diagnostics).
#[cfg(test)]
pub(super) fn scc_index_map(graph: &EscapeCallGraph) -> rustc_hash::FxHashMap<EscapeFunId, usize> {
    let mut m = rustc_hash::FxHashMap::default();
    for (i, comp) in graph.sccs().into_iter().enumerate() {
        for id in comp {
            m.insert(id, i);
        }
    }
    m
}

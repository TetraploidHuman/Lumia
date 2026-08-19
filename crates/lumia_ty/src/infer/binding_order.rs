//! Top-level binding order: SCCs so polymorphic `let` generalizes before use.

use lumia_hir::{Item, Module};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

use super::free_vars::free_var_names;

fn binding_name(item: &Item) -> Option<&str> {
    match item {
        Item::Fun(f) => Some(f.name.as_str()),
        Item::Val { name, .. } => Some(name.as_str()),
    }
}

fn binding_frees(item: &Item) -> HashSet<String> {
    match item {
        Item::Fun(f) => free_var_names(&f.body),
        Item::Val { body, .. } => free_var_names(body),
    }
}

/// Indices of top-level Fun/Val items in dependency SCC order (each SCC is
/// a list of item indices, sorted by source order within the component).
pub(super) fn binding_sccs(module: &Module) -> Vec<Vec<usize>> {
    let mut name_to_idx: HashMap<String, usize> = HashMap::default();
    let mut nodes: Vec<usize> = Vec::new();
    for (i, item) in module.items.iter().enumerate() {
        if let Some(name) = binding_name(item) {
            name_to_idx.insert(name.to_string(), i);
            nodes.push(i);
        }
    }
    if nodes.is_empty() {
        return Vec::new();
    }

    // Edge i → j means j depends on i (i must be typed before / with j).
    let mut succ: HashMap<usize, Vec<usize>> = HashMap::default();
    for &i in &nodes {
        succ.entry(i).or_default();
        let frees = binding_frees(&module.items[i]);
        for name in frees {
            if let Some(&j) = name_to_idx.get(&name) {
                if j != i {
                    // i references j → j before i: edge j → i
                    succ.entry(j).or_default().push(i);
                }
            }
            // UFCS: free short method name depends on every mangled instance body.
            if module.method_traits.keys().any(|k| k.as_str() == name) {
                for ((_, method), mangleds) in &module.trait_methods {
                    if method.as_str() != name {
                        continue;
                    }
                    for m in mangleds {
                        if let Some(&j) = name_to_idx.get(m.as_str()) {
                            if j != i {
                                succ.entry(j).or_default().push(i);
                            }
                        }
                    }
                }
            }
        }
    }

    // Tarjan SCC
    let mut index = 0usize;
    let mut stack: Vec<usize> = Vec::new();
    let mut on_stack: HashSet<usize> = HashSet::default();
    let mut indices: HashMap<usize, usize> = HashMap::default();
    let mut lowlink: HashMap<usize, usize> = HashMap::default();
    let mut sccs: Vec<Vec<usize>> = Vec::new();

    fn strongconnect(
        v: usize,
        succ: &HashMap<usize, Vec<usize>>,
        index: &mut usize,
        stack: &mut Vec<usize>,
        on_stack: &mut HashSet<usize>,
        indices: &mut HashMap<usize, usize>,
        lowlink: &mut HashMap<usize, usize>,
        sccs: &mut Vec<Vec<usize>>,
    ) {
        indices.insert(v, *index);
        lowlink.insert(v, *index);
        *index += 1;
        stack.push(v);
        on_stack.insert(v);

        for &w in succ.get(&v).into_iter().flatten() {
            if !indices.contains_key(&w) {
                strongconnect(w, succ, index, stack, on_stack, indices, lowlink, sccs);
                let lw = *lowlink.get(&w).unwrap();
                let lv = *lowlink.get(&v).unwrap();
                lowlink.insert(v, lv.min(lw));
            } else if on_stack.contains(&w) {
                let iw = *indices.get(&w).unwrap();
                let lv = *lowlink.get(&v).unwrap();
                lowlink.insert(v, lv.min(iw));
            }
        }

        if lowlink.get(&v) == indices.get(&v) {
            let mut comp = Vec::new();
            loop {
                let w = stack.pop().expect("tarjan stack");
                on_stack.remove(&w);
                comp.push(w);
                if w == v {
                    break;
                }
            }
            comp.sort_unstable();
            sccs.push(comp);
        }
    }

    for &v in &nodes {
        if !indices.contains_key(&v) {
            strongconnect(
                v,
                &succ,
                &mut index,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlink,
                &mut sccs,
            );
        }
    }

    // Tarjan emits SCCs in reverse topo order; reverse for dependency-first.
    sccs.reverse();
    sccs
}

#[cfg(test)]
mod tests {
    use super::binding_sccs;
    use lumia_hir::{Expr, Fun, Item, Module};
    use lumia_syntax::Span;

    fn fun(name: &str, body: Expr) -> Item {
        Item::Fun(Fun {
            name: name.into(),
            params: vec!["x".into()],
            param_ann: vec![None],
            ret_ann: None,
            body,
            span: Span::dummy(),
            is_main: false,
            external: None,
            foreign_sig: None,
            foreign_pure: false,
            is_priv: false,
        })
    }

    #[test]
    fn acyclic_callee_before_caller() {
        // use → id  ⇒  process id SCC before use
        let id = fun("id", Expr::Var("x".into(), Span::dummy()));
        let use_ = fun(
            "use",
            Expr::Call {
                callee: Box::new(Expr::Var("id".into(), Span::dummy())),
                args: vec![Expr::Int(1, Span::dummy())],
                span: Span::dummy(),
            },
        );
        // Source order: use then id
        let m = Module {
            name: "M".into(),
            items: vec![use_, id],
            adts: vec![],
            products: vec![],
            instances: Default::default(),
            trait_methods: Default::default(),
            method_traits: Default::default(),
        };
        let sccs = binding_sccs(&m);
        let flat: Vec<&str> = sccs
            .iter()
            .flatten()
            .map(|&i| match &m.items[i] {
                Item::Fun(f) => f.name.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(flat, vec!["id", "use"]);
    }
}

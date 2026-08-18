//! TCO / musttail / SCC analysis.

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::BasicMetadataValueEnum;
use lumia_core::{Block, CoreModule, Local, Op, Value};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

impl<'ctx> Codegen<'ctx> {
    /// Emit `musttail call` + `ret` for pure Int TCO (self or mutual; no GC roots live).
    /// Returns true if the call was emitted as a terminator.
    pub(crate) fn emit_musttail_call(&mut self, fun: &str, args: &[Local]) -> Result<bool> {
        let callee = match self.funs.functions.get(fun).copied() {
            Some(f) => f,
            None => return Ok(false),
        };
        let mut av: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
        for a in args {
            av.push(self.coerce_i64(self.local(*a)?)?.into());
        }
        let call = crate::error::llvm(self.llvm.builder.build_call(callee, &av, "tco"))?;
        call.set_tail_call_kind(inkwell::values::LLVMTailCallKind::LLVMTailCallKindMustTail);
        // Lumia ABI always returns i64 (Unit is the zero word). A void LLVM
        // result here means the callee declaration drifted from Core.
        let ret = call
            .try_as_basic_value()
            .basic()
            .with_context(|| {
                format!("ICE: musttail call to `{fun}` returned void; expected i64 ABI value")
            })?
            .into_int_value();
        // No root epilogue: musttail requires call immediately followed by ret.
        debug_assert_eq!(self.frame.root_depth, 0);
        crate::error::llvm(self.llvm.builder.build_return(Some(&ret)))?;
        Ok(true)
    }
}

/// Types allowed on pure TCO SCCs (DESIGN §4.4). Heap params OK: entry re-roots;
/// callers `root_pop_to(0)` immediately before musttail. Closures stay out.
fn tco_eligible_ty(t: &Type) -> bool {
    match t {
        Type::Int | Type::Bool | Type::Float | Type::Var(_) => true,
        Type::String | Type::Char | Type::Unit => true,
        Type::List(_)
        | Type::Set(_)
        | Type::Map(_, _)
        | Type::Task(_)
        | Type::Channel(_)
        | Type::Adt { .. }
        | Type::Tuple(_)
        | Type::TuplePrefix(_) => true,
        Type::Fun(_, _, _) => false,
    }
}

pub(crate) fn compute_tco_sccs(core: &CoreModule) -> HashMap<String, HashSet<String>> {
    let eligible: HashSet<String> = core
        .functions
        .iter()
        .filter(|f| {
            // DESIGN §4.4: pure mutual recursion is guaranteed; IO is not required
            // to TCO, but eligible Int/heap-param SCCs still get musttail when the
            // recursive edge is a direct/FunRef call (IO on other arms is fine).
            f.memo.is_none()
                && f.external.is_none()
                && tco_eligible_ty(&f.ret_ty)
                && f.param_tys.iter().all(tco_eligible_ty)
        })
        .map(|f| f.name.clone())
        .collect();
    if eligible.is_empty() {
        return HashMap::default();
    }
    let mut graph: HashMap<String, HashSet<String>> = HashMap::default();
    for name in &eligible {
        graph.insert(name.clone(), HashSet::default());
    }
    for f in &core.functions {
        if !eligible.contains(&f.name) {
            continue;
        }
        let mut callees = HashSet::default();
        collect_direct_calls(&f.body, &mut callees);
        for c in callees {
            if eligible.contains(&c) {
                // `f.name` was inserted into `graph` when building `eligible`.
                graph.entry(f.name.clone()).or_default().insert(c);
            }
        }
    }
    // Tarjan SCC
    let mut index = 0u32;
    let mut stack: Vec<String> = Vec::new();
    let mut on_stack: HashSet<String> = HashSet::default();
    let mut indices: HashMap<String, u32> = HashMap::default();
    let mut lowlink: HashMap<String, u32> = HashMap::default();
    let mut sccs: Vec<HashSet<String>> = Vec::new();

    fn strongconnect(
        v: &str,
        graph: &HashMap<String, HashSet<String>>,
        index: &mut u32,
        stack: &mut Vec<String>,
        on_stack: &mut HashSet<String>,
        indices: &mut HashMap<String, u32>,
        lowlink: &mut HashMap<String, u32>,
        sccs: &mut Vec<HashSet<String>>,
    ) {
        indices.insert(v.to_string(), *index);
        lowlink.insert(v.to_string(), *index);
        *index += 1;
        stack.push(v.to_string());
        on_stack.insert(v.to_string());
        if let Some(ns) = graph.get(v) {
            for w in ns {
                if !indices.contains_key(w) {
                    strongconnect(w, graph, index, stack, on_stack, indices, lowlink, sccs);
                    let lw = *lowlink
                        .get(w)
                        .expect("ICE: Tarjan lowlink missing after recurse");
                    let lv = *lowlink
                        .get(v)
                        .expect("ICE: Tarjan lowlink missing for current");
                    lowlink.insert(v.to_string(), lv.min(lw));
                } else if on_stack.contains(w) {
                    let iw = *indices
                        .get(w)
                        .expect("ICE: Tarjan index missing for on-stack neighbor");
                    let lv = *lowlink
                        .get(v)
                        .expect("ICE: Tarjan lowlink missing for current");
                    lowlink.insert(v.to_string(), lv.min(iw));
                }
            }
        }
        if lowlink.get(v) == indices.get(v) {
            let mut comp = HashSet::default();
            loop {
                let w = stack.pop().expect("ICE: Tarjan SCC pop on empty stack");
                on_stack.remove(&w);
                comp.insert(w.clone());
                if w == v {
                    break;
                }
            }
            // Keep SCCs that can recurse (size>1 or self-loop).
            let self_loop = graph.get(v).map(|s| s.contains(v)).unwrap_or(false);
            if comp.len() > 1 || self_loop {
                sccs.push(comp);
            }
        }
    }

    let nodes: Vec<String> = eligible.iter().cloned().collect();
    for n in nodes {
        if !indices.contains_key(&n) {
            strongconnect(
                &n,
                &graph,
                &mut index,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlink,
                &mut sccs,
            );
        }
    }

    let mut out = HashMap::default();
    for scc in sccs {
        for m in &scc {
            out.insert(m.clone(), scc.clone());
        }
    }
    out
}

fn collect_direct_calls(block: &Block, out: &mut HashSet<String>) {
    collect_calls_with_funrefs(block, &HashMap::default(), out);
}

/// Collect direct callees, resolving `FunRef` → `IndirectCall` (for TCO SCCs).
fn collect_calls_with_funrefs(
    block: &Block,
    parent_funrefs: &HashMap<u32, String>,
    out: &mut HashSet<String>,
) {
    let mut funref_of = parent_funrefs.clone();
    for op in &block.ops {
        let Op::Let { local, value, .. } = op else {
            continue;
        };
        match value {
            Value::Call { fun, .. } => {
                out.insert(fun.name.clone());
            }
            Value::IndirectCall { callee, .. } => {
                if let Some(fun) = funref_of.get(&callee.0) {
                    out.insert(fun.clone());
                }
            }
            Value::If {
                then_block,
                else_block,
                ..
            } => {
                collect_calls_with_funrefs(then_block, &funref_of, out);
                collect_calls_with_funrefs(else_block, &funref_of, out);
            }
            Value::Loop {
                header,
                body,
                latch,
            } => {
                collect_calls_with_funrefs(header, &funref_of, out);
                collect_calls_with_funrefs(body, &funref_of, out);
                collect_calls_with_funrefs(latch, &funref_of, out);
            }
            _ => {}
        }
        if let Value::FunRef(name) = value {
            funref_of.insert(local.0, name.name.clone());
        } else if let Value::Local(Local(src)) = value {
            if let Some(n) = funref_of.get(src).cloned() {
                funref_of.insert(local.0, n);
            } else {
                funref_of.remove(&local.0);
            }
        } else {
            funref_of.remove(&local.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{Block, CoreFun, CoreModule, FunKind, Local, Op, Value};
    use lumia_ty::{Effect, Type};

    fn fun(name: &str, body: Block, self_call: Option<&str>) -> CoreFun {
        let mut ops = body.ops;
        if let Some(callee) = self_call {
            ops.push(Op::Let {
                local: Local(10),
                value: Value::Call {
                    fun: callee.into(),
                    args: vec![Local(0)],
                },
                pure_region: true,
            });
        }
        CoreFun {
            name: name.into(),
            params: vec![Local(0)],
            param_names: vec!["n".into()],
            param_tys: vec![Type::Int],
            body: Block {
                ops,
                result: Some(Local(0)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        }
    }

    #[test]
    fn tco_scc_self_recursive() {
        let core = CoreModule::with_functions(
            "M",
            vec![fun(
                "sum",
                Block {
                    ops: vec![],
                    result: None,
                },
                Some("sum"),
            )],
        );
        let sccs = compute_tco_sccs(&core);
        assert!(
            sccs.contains_key("sum"),
            "sum should form a TCO SCC: {sccs:?}"
        );
        assert!(sccs["sum"].contains("sum"));
    }

    #[test]
    fn tco_scc_mutual_recursion() {
        let even = fun(
            "even",
            Block {
                ops: vec![],
                result: None,
            },
            Some("odd"),
        );
        let odd = fun(
            "odd",
            Block {
                ops: vec![],
                result: None,
            },
            Some("even"),
        );
        let core = CoreModule::with_functions("M", vec![even, odd]);
        let sccs = compute_tco_sccs(&core);
        assert!(sccs.contains_key("even"));
        assert!(sccs["even"].contains("odd"));
        assert!(sccs["odd"].contains("even"));
    }

    #[test]
    fn tco_scc_excludes_external() {
        let mut f = fun(
            "sum",
            Block {
                ops: vec![],
                result: None,
            },
            Some("sum"),
        );
        f.external = Some("c_sum".into());
        f.foreign_abi = lumia_core::ForeignAbi::C;
        let core = CoreModule::with_functions("M", vec![f]);
        let sccs = compute_tco_sccs(&core);
        assert!(
            sccs.is_empty(),
            "external fun must not enter TCO SCCs: {sccs:?}"
        );
    }
}

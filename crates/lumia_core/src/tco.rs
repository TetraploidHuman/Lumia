//! Tail-call optimization (TCO) SCC analysis (DESIGN §4.4).
//!
//! Pure self/mutual recursion on eligible scalar/container ABIs forms SCCs;
//! codegen emits `musttail` when the recursive edge is a direct or FunRef-resolved call.

use crate::ir::{Block, CoreModule, Local, Value};
use crate::{collect_funref_callees, find_top_level_local_def, FunRefAliases};
use lumia_syntax::Sym;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Resolved tail-position recursive call for musttail emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcoTailCall {
    pub fun: Sym,
    pub args: Vec<Local>,
}

/// Resolve a tail-position [`Value`] to a direct TCO callee within `block`.
///
/// Handles `Call`, FunRef-resolved `IndirectCall`, and trivial SSA alias chains
/// (`let t = f(...); t` → peel `Local` to the underlying `Call`). Uses `aliases`
/// as of *before* the current Let (emit notes the binding after a successful musttail).
pub fn resolve_tco_callee(
    block: &Block,
    value: &Value,
    peers: &HashSet<Sym>,
    aliases: &FunRefAliases,
    seen_locals: &mut HashSet<u32>,
) -> Option<(Sym, Vec<Local>)> {
    match value {
        Value::Call { fun, args } if peers.contains(fun.as_str()) => {
            Some((fun.name.clone(), args.clone()))
        }
        Value::IndirectCall { callee, args } => {
            let fun = Sym::from(aliases.resolve(callee.0)?);
            peers.contains(fun.as_str()).then(|| (fun, args.clone()))
        }
        Value::Local(Local(id)) => {
            if !seen_locals.insert(*id) {
                return None;
            }
            let inner = find_top_level_local_def(block, *id)?;
            resolve_tco_callee(block, inner, peers, aliases, seen_locals)
        }
        _ => None,
    }
}

/// Shorthand for [`resolve_tco_callee`] with a fresh local cycle guard.
pub fn resolve_tco_callee_fresh(
    block: &Block,
    value: &Value,
    peers: &HashSet<Sym>,
    aliases: &FunRefAliases,
) -> Option<(Sym, Vec<Local>)> {
    resolve_tco_callee(block, value, peers, aliases, &mut HashSet::default())
}

/// Like [`resolve_tco_callee_fresh`] but returns a structured [`TcoTailCall`].
pub fn resolve_tco_tail_call(
    block: &Block,
    value: &Value,
    peers: &HashSet<Sym>,
    aliases: &FunRefAliases,
) -> Option<TcoTailCall> {
    resolve_tco_callee_fresh(block, value, peers, aliases)
        .map(|(fun, args)| TcoTailCall { fun, args })
}

/// Types allowed on pure TCO SCCs (DESIGN §4.4). Heap params OK: entry re-roots;
/// callers `root_pop_to(0)` immediately before musttail. Closures stay out.
fn tco_eligible_ty(t: &Type) -> bool {
    match t {
        Type::Int | Type::Bool | Type::Float | Type::Var(_) | Type::Unknown => true,
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

/// Map each TCO-eligible function name to its full mutual-recursion peer set.
pub fn compute_tco_sccs(core: &CoreModule) -> HashMap<Sym, HashSet<Sym>> {
    let eligible: HashSet<Sym> = core
        .functions
        .iter()
        .filter(|f| {
            // DESIGN §4.4: pure mutual recursion is guaranteed; IO is not required
            // to TCO, but eligible Int/Float/heap-param SCCs still get musttail when
            // the recursive edge is a direct/FunRef call (IO on other arms is fine).
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
    let mut graph: HashMap<Sym, HashSet<Sym>> = HashMap::default();
    for name in &eligible {
        graph.insert(name.clone(), HashSet::default());
    }
    for f in &core.functions {
        if !eligible.contains(f.name.as_str()) {
            continue;
        }
        let mut callees = HashSet::default();
        collect_funref_callees(&f.body, &mut callees);
        for c in callees {
            if eligible.contains(c.as_str()) {
                graph.entry(f.name.clone()).or_default().insert(c);
            }
        }
    }
    tarjan_sccs(&graph, &eligible)
}

fn tarjan_sccs(
    graph: &HashMap<Sym, HashSet<Sym>>,
    eligible: &HashSet<Sym>,
) -> HashMap<Sym, HashSet<Sym>> {
    let mut index = 0u32;
    let mut stack: Vec<Sym> = Vec::new();
    let mut on_stack: HashSet<Sym> = HashSet::default();
    let mut indices: HashMap<Sym, u32> = HashMap::default();
    let mut lowlink: HashMap<Sym, u32> = HashMap::default();
    let mut sccs: Vec<HashSet<Sym>> = Vec::new();

    fn strongconnect(
        v: &Sym,
        graph: &HashMap<Sym, HashSet<Sym>>,
        index: &mut u32,
        stack: &mut Vec<Sym>,
        on_stack: &mut HashSet<Sym>,
        indices: &mut HashMap<Sym, u32>,
        lowlink: &mut HashMap<Sym, u32>,
        sccs: &mut Vec<HashSet<Sym>>,
    ) {
        indices.insert(v.clone(), *index);
        lowlink.insert(v.clone(), *index);
        *index += 1;
        stack.push(v.clone());
        on_stack.insert(v.clone());
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
                    lowlink.insert(v.clone(), lv.min(lw));
                } else if on_stack.contains(w) {
                    let iw = *indices
                        .get(w)
                        .expect("ICE: Tarjan index missing for on-stack neighbor");
                    let lv = *lowlink
                        .get(v)
                        .expect("ICE: Tarjan lowlink missing for current");
                    lowlink.insert(v.clone(), lv.min(iw));
                }
            }
        }
        if lowlink.get(v) == indices.get(v) {
            let mut comp = HashSet::default();
            loop {
                let w = stack.pop().expect("ICE: Tarjan SCC pop on empty stack");
                on_stack.remove(&w);
                comp.insert(w.clone());
                if &w == v {
                    break;
                }
            }
            let self_loop = graph.get(v).map(|s| s.contains(v)).unwrap_or(false);
            if comp.len() > 1 || self_loop {
                sccs.push(comp);
            }
        }
    }

    for n in eligible.iter().cloned().collect::<Vec<_>>() {
        if !indices.contains_key(&n) {
            strongconnect(
                &n,
                graph,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Block, CoreFun, FunKind, Local, Op, Value};
    use crate::{find_top_level_local_def, FunRefAliases, FunRefAlloc};
    use lumia_ty::{Effect, Type};

    fn fun(
        name: &str,
        body: Block,
        self_call: Option<&str>,
        ret_ty: Type,
        param_tys: Vec<Type>,
    ) -> CoreFun {
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
            param_tys,
            body: Block {
                ops,
                result: Some(Local(0)),
            },
            ret_ty,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        }
    }

    fn int_fun(name: &str, body: Block, self_call: Option<&str>) -> CoreFun {
        fun(name, body, self_call, Type::Int, vec![Type::Int])
    }

    #[test]
    fn tco_scc_self_recursive() {
        let core = CoreModule::with_functions(
            "M",
            vec![int_fun(
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
    fn tco_scc_float_self_recursive() {
        let core = CoreModule::with_functions(
            "M",
            vec![fun(
                "sumTo",
                Block {
                    ops: vec![],
                    result: None,
                },
                Some("sumTo"),
                Type::Float,
                vec![Type::Float, Type::Float],
            )],
        );
        let sccs = compute_tco_sccs(&core);
        assert!(
            sccs.contains_key("sumTo"),
            "Float sumTo should form a TCO SCC: {sccs:?}"
        );
        assert!(sccs["sumTo"].contains("sumTo"));
    }

    #[test]
    fn tco_scc_mutual_recursion() {
        let even = int_fun(
            "even",
            Block {
                ops: vec![],
                result: None,
            },
            Some("odd"),
        );
        let odd = int_fun(
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
        let mut f = int_fun(
            "sum",
            Block {
                ops: vec![],
                result: None,
            },
            Some("sum"),
        );
        f.external = Some("c_sum".into());
        f.foreign_abi = crate::ForeignAbi::C;
        let core = CoreModule::with_functions("M", vec![f]);
        let sccs = compute_tco_sccs(&core);
        assert!(
            sccs.is_empty(),
            "external fun must not enter TCO SCCs: {sccs:?}"
        );
    }

    #[test]
    fn tco_scc_sees_funref_via_named_slot() {
        let even_body = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::FunRef("odd".into()),
                    pure_region: true,
                },
                Op::Assign {
                    name: "next".into(),
                    value: Local(1),
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Name("next".into()),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::IndirectCall {
                        callee: Local(2),
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(3)),
        };
        let odd = int_fun(
            "odd",
            Block {
                ops: vec![],
                result: None,
            },
            Some("even"),
        );
        let even = CoreFun {
            name: "even".into(),
            params: vec![Local(0)],
            param_names: vec!["n".into()],
            param_tys: vec![Type::Int],
            body: even_body,
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        };
        let core = CoreModule::with_functions("M", vec![even, odd]);
        let sccs = compute_tco_sccs(&core);
        assert!(
            sccs.get("even").is_some_and(|s| s.contains("odd")),
            "slot FunRef IndirectCall must enter TCO SCC: {sccs:?}"
        );
        assert!(sccs.get("odd").is_some_and(|s| s.contains("even")));
    }

    #[test]
    fn tco_scc_float_sum_to_from_pipeline() {
        let src = r#"
module M
val sumTo(n, acc) = {
    if n == 0.0 { acc } else { sumTo(n - 1.0, acc + n) }
}
val main = { sumTo(10.0, 0.0) }
"#;
        let core = crate::compile_source_to_core(src).expect("core");
        let sum_to = core
            .functions
            .iter()
            .find(|f| f.name == "sumTo")
            .expect("sumTo in core");
        assert!(matches!(sum_to.ret_ty, Type::Float));
        let sccs = compute_tco_sccs(&core);
        assert!(
            sccs.get("sumTo").is_some_and(|s| s.contains("sumTo")),
            "pipeline Float sumTo must enter TCO SCC: {sccs:?}"
        );
    }

    #[test]
    fn resolve_tco_callee_peels_local_alias_to_call() {
        let peers: HashSet<Sym> = [Sym::from("sum")].into_iter().collect();
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Call {
                        fun: "sum".into(),
                        args: vec![Local(0), Local(2)],
                    },
                    pure_region: true,
                },
                Op::Let {
                    local: Local(3),
                    value: Value::Local(Local(1)),
                    pure_region: true,
                },
            ],
            result: Some(Local(3)),
        };
        let tail = find_top_level_local_def(&block, 3).expect("tail def");
        let (fun, args) =
            resolve_tco_callee_fresh(&block, tail, &peers, &FunRefAliases::default()).expect("tco");
        assert_eq!(fun.as_str(), "sum");
        assert_eq!(args, vec![Local(0), Local(2)]);
    }

    #[test]
    fn resolve_tco_callee_indirect_via_slot() {
        let peers: HashSet<Sym> = [Sym::from("odd")].into_iter().collect();
        let mut aliases = FunRefAliases::default();
        aliases.note_let(0, &Value::FunRef("odd".into()), FunRefAlloc::Ignore, None);
        aliases.note_assign(lumia_hir::Sym::from("next"), Local(0));
        aliases.note_let(1, &Value::Name("next".into()), FunRefAlloc::Ignore, None);
        let block = Block {
            ops: vec![Op::Let {
                local: Local(2),
                value: Value::IndirectCall {
                    callee: Local(1),
                    args: vec![Local(0)],
                },
                pure_region: true,
            }],
            result: Some(Local(2)),
        };
        let tail = find_top_level_local_def(&block, 2).expect("tail");
        let (fun, _) = resolve_tco_callee_fresh(&block, tail, &peers, &aliases).expect("tco");
        assert_eq!(fun.as_str(), "odd");
    }

    #[test]
    fn resolve_tco_tail_call_via_return_local() {
        let peers: HashSet<Sym> = [Sym::from("sum")].into_iter().collect();
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(1),
                    value: Value::Call {
                        fun: "sum".into(),
                        args: vec![Local(0), Local(2)],
                    },
                    pure_region: true,
                },
                Op::Return { value: Local(1) },
            ],
            result: None,
        };
        let tail = resolve_tco_tail_call(
            &block,
            &Value::Local(Local(1)),
            &peers,
            &FunRefAliases::default(),
        )
        .expect("return local must peel to call");
        assert_eq!(tail.fun, "sum");
        assert_eq!(tail.args, vec![Local(0), Local(2)]);
    }

    #[test]
    fn tco_return_tail_sum_from_pipeline() {
        let src = r#"
module M
val sum(n, acc) = {
    if n == 0 { return acc } else { return sum(n - 1, acc + n) }
}
val main = { sum(10, 0) }
"#;
        let core = crate::compile_source_to_core(src).expect("core");
        let sum = core
            .functions
            .iter()
            .find(|f| f.name == "sum")
            .expect("sum");
        let peers = compute_tco_sccs(&core)
            .get("sum")
            .cloned()
            .expect("sum scc");
        let else_block = sum
            .body
            .ops
            .iter()
            .find_map(|op| {
                let Op::Let {
                    value: Value::If { else_block, .. },
                    ..
                } = op
                else {
                    return None;
                };
                Some(else_block.as_ref())
            })
            .expect("if else");
        let Op::Return { value } = else_block.ops.last().expect("return tail") else {
            panic!("else arm must end with return");
        };
        let tail = resolve_tco_tail_call(
            else_block,
            &Value::Local(*value),
            &peers,
            &FunRefAliases::default(),
        )
        .expect("return tail must resolve to sum");
        assert_eq!(tail.fun, "sum");
    }

    #[test]
    fn tco_alias_tail_sum_from_pipeline() {
        let src = r#"
module M
val sum(n, acc) = {
    if n == 0 { acc } else {
        val t = sum(n - 1, acc + n)
        t
    }
}
val main = { sum(10, 0) }
"#;
        let core = crate::compile_source_to_core(src).expect("core");
        let sum = core
            .functions
            .iter()
            .find(|f| f.name == "sum")
            .expect("sum");
        let peers = compute_tco_sccs(&core)
            .get("sum")
            .cloned()
            .expect("sum scc");
        // Else arm ends with `let t = Local(call_result); t` — must peel to Call.
        let else_block = sum
            .body
            .ops
            .iter()
            .find_map(|op| {
                let Op::Let {
                    value: Value::If { else_block, .. },
                    ..
                } = op
                else {
                    return None;
                };
                Some(else_block.as_ref())
            })
            .expect("if else");
        let Local(r) = else_block.result.expect("else result");
        let tail = find_top_level_local_def(else_block, r).expect("tail value");
        let (fun, _) =
            resolve_tco_callee_fresh(else_block, tail, &peers, &FunRefAliases::default())
                .expect("alias tail must resolve to sum");
        assert_eq!(fun.as_str(), "sum");
    }
}

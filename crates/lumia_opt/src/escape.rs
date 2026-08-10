//! Escape analysis (DESIGN §7.2 Escape Analysis).
//!
//! Conservative: a local escapes if it may be observed after the current
//! function returns, stored into a heap object that escapes, passed to an
//! unknown callee, or written into a named binding.
//!
//! Direct calls to known functions only mark args whose corresponding
//! formals escape in the callee (fixed-point summaries).

use lumia_core::{Block, CoreFun, CoreModule, Local, Op, Value};
use lumia_hir::Builtin;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Per-function: which parameter indices escape from the callee.
type ParamEscape = Vec<bool>;

/// Locals that may outlive their defining region / be observed from outside.
pub fn escaping_locals(fun: &CoreFun) -> HashSet<Local> {
    escaping_locals_with(fun, &HashMap::default())
        .into_iter()
        .collect()
}

fn escaping_locals_with(fun: &CoreFun, summaries: &HashMap<String, ParamEscape>) -> HashSet<Local> {
    let mut escaping: HashSet<Local> = HashSet::default();
    seed_escaping(&fun.body, &mut escaping, summaries);
    let mut changed = true;
    while changed {
        changed = false;
        changed |= propagate_block(&fun.body, &mut escaping);
    }
    escaping
}

/// Escape analysis: write results onto each [`CoreFun::escaping`] for later passes.
pub struct EscapePass;

impl crate::Pass for EscapePass {
    fn name(&self) -> &str {
        "escape"
    }
    fn run(&self, module: &mut CoreModule) {
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
            break;
        }
    }
    summaries
}

fn seed_escaping(
    block: &Block,
    escaping: &mut HashSet<Local>,
    summaries: &HashMap<String, ParamEscape>,
) {
    if let Some(r) = block.result {
        escaping.insert(r);
    }
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                seed_value(value, escaping, summaries)
            }
            Op::Assign { value, .. } => {
                // Named bindings are visible across the function; treat as escaping.
                escaping.insert(*value);
            }
            Op::Return { value } => {
                // Early return leaves the function — same as `block.result`.
                escaping.insert(*value);
            }
            Op::Break | Op::Continue => {}
        }
    }
}

fn seed_value(
    value: &Value,
    escaping: &mut HashSet<Local>,
    summaries: &HashMap<String, ParamEscape>,
) {
    match value {
        Value::Call { fun, args } => {
            if let Some(pe) = summaries.get(fun) {
                for (i, a) in args.iter().enumerate() {
                    // Missing summary slots → conservative escape.
                    if pe.get(i).copied().unwrap_or(true) {
                        escaping.insert(*a);
                    }
                }
            } else {
                for a in args {
                    escaping.insert(*a);
                }
            }
        }
        Value::IndirectCall { callee, args } => {
            escaping.insert(*callee);
            for a in args {
                escaping.insert(*a);
            }
        }
        Value::Builtin { name, args } => {
            if builtin_may_capture(*name) {
                for a in args {
                    escaping.insert(*a);
                }
            } else if matches!(*name, Builtin::Show) {
                // `lumia_show` requires a heap payload; Lit* stack objects print as ints.
                for a in args {
                    escaping.insert(*a);
                }
            } else if matches!(*name, Builtin::ListGet | Builtin::Contains) {
                // Collection is not retained, but Map/Set *keys* must be heap
                // objects: `lumia_eq` rejects non-heap payloads (`is_heap_payload`).
                if let Some(k) = args.get(1) {
                    escaping.insert(*k);
                }
            }
        }
        Value::FunRef(_) => {}
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            seed_escaping(then_block, escaping, summaries);
            seed_escaping(else_block, escaping, summaries);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            seed_escaping(header, escaping, summaries);
            seed_escaping(body, escaping, summaries);
            seed_escaping(latch, escaping, summaries);
        }
        Value::Lambda { body, .. } => seed_escaping(body, escaping, summaries),
        _ => {}
    }
}

fn builtin_may_capture(b: Builtin) -> bool {
    // Conservative: anything that builds / mutates heap collections or does I/O
    // with a value may retain it. Pure projections (len/get/tag) do not.
    // `Show` is handled in `seed_escaping`: it does not retain after return, but
    // the operand must be a heap object for `lumia_show`.
    !matches!(
        b,
        Builtin::ListLen
            | Builtin::ListGet
            | Builtin::AdtTag
            | Builtin::AdtField
            | Builtin::Contains
            | Builtin::Show
            | Builtin::MatchFail
    )
}

fn propagate_block(block: &Block, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                changed |= propagate_let(*local, value, escaping);
            }
            Op::Effect { value } => {
                changed |= propagate_value_only(value, escaping);
            }
            Op::Assign { .. } | Op::Break | Op::Continue | Op::Return { .. } => {}
        }
    }
    changed
}

fn propagate_let(local: Local, value: &Value, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    // If the binding escapes, everything it aliases / contains escapes.
    if escaping.contains(&local) {
        changed |= mark_inputs_escaping(value, escaping);
    }
    changed |= propagate_value_only(value, escaping);
    changed
}

fn propagate_value_only(value: &Value, escaping: &mut HashSet<Local>) -> bool {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            let mut c = propagate_block(then_block, escaping);
            c |= propagate_block(else_block, escaping);
            c
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            let mut c = propagate_block(header, escaping);
            c |= propagate_block(body, escaping);
            c |= propagate_block(latch, escaping);
            c
        }
        Value::Lambda { body, .. } => propagate_block(body, escaping),
        _ => false,
    }
}

fn mark_inputs_escaping(value: &Value, escaping: &mut HashSet<Local>) -> bool {
    let mut changed = false;
    let mut mark = |l: Local| {
        if escaping.insert(l) {
            changed = true;
        }
    };
    match value {
        Value::Local(l) => mark(*l),
        Value::Binary { left, right, .. } => {
            mark(*left);
            mark(*right);
        }
        Value::Unary { operand, .. } => mark(*operand),
        Value::Builtin { name, args } => {
            // Pure projections do not retain the collection; returning `xs.len()`
            // must not mark `xs` itself as escaping.
            if builtin_may_capture(*name) {
                for a in args {
                    mark(*a);
                }
            }
        }
        // `Call` args are seeded from callee param-escape summaries only.
        // A escaping Call *result* does not imply args escape (unless a formal
        // aliases the return — already reflected in the summary).
        Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args, .. }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure { captures: args, .. } => {
            for a in args {
                mark(*a);
            }
        }
        Value::Call { .. } => {}
        Value::IndirectCall { callee, args } => {
            mark(*callee);
            for a in args {
                mark(*a);
            }
        }
        Value::ClosureCap { env, .. } => mark(*env),
        Value::If { cond, .. } => mark(*cond),
        Value::Name(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_)
        | Value::Loop { .. }
        | Value::Lambda { .. } => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pass;
    use lumia_core::{Block, CoreFun, Op, Value};
    use lumia_ty::{Effect, Type};

    fn fun_with_body(body: Block) -> CoreFun {
        CoreFun {
            name: "f".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body,
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        }
    }

    #[test]
    fn result_escapes() {
        let body = Block {
            params: vec![],
            ops: vec![Op::Let {
                local: Local(0),
                value: Value::Int(1),
                pure_region: true,
            }],
            result: Some(Local(0)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(0)));
    }

    #[test]
    fn early_return_escapes() {
        let body = Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Return { value: Local(0) },
            ],
            result: None,
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(
            esc.contains(&Local(0)),
            "early-return payload must escape (no stack LitAdt)"
        );
    }

    #[test]
    fn dead_temp_does_not_escape() {
        let body = Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Int(2),
                    pure_region: true,
                },
            ],
            result: Some(Local(1)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(1)));
        assert!(!esc.contains(&Local(0)));
    }

    #[test]
    fn call_args_escape() {
        let body = Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(1),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Call {
                        fun: "g".into(),
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(1)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(0)));
        assert!(esc.contains(&Local(1)));
    }

    #[test]
    fn known_pure_len_callee_does_not_escape_arg() {
        use lumia_core::{ListRepr, Op};
        let len_fun = CoreFun {
            name: "len".into(),
            params: vec![Local(0)],
            param_names: vec!["xs".into()],
            param_tys: vec![Type::List(Box::new(Type::Int))],
            body: Block {
                params: vec![],
                ops: vec![Op::Let {
                    local: Local(1),
                    value: Value::Builtin {
                        name: Builtin::ListLen,
                        args: vec![Local(0)],
                    },
                    pure_region: true,
                }],
                result: Some(Local(1)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        };
        let main_fun = CoreFun {
            name: "main".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body: Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::Int(1),
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::AllocList {
                            elems: vec![Local(0)],
                            repr: ListRepr::HeapList,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(2),
                        value: Value::Call {
                            fun: "len".into(),
                            args: vec![Local(1)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(2)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: true,
            memo: None,
            external: None,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
        };
        let mut module = CoreModule {
            name: "M".into(),
            functions: vec![len_fun, main_fun],
            hash_adts: HashSet::default(),
            trait_methods: HashMap::default(),
        };
        EscapePass.run(&mut module);
        let main = module.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(
            !main.escaping.contains(&Local(1)),
            "list passed only to pure len must not escape: {:?}",
            main.escaping
        );
    }

    #[test]
    fn alloc_elems_escape_when_list_returned() {
        let body = Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(7),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::AllocList {
                        elems: vec![Local(0)],
                        repr: lumia_core::ListRepr::HeapList,
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(1)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(esc.contains(&Local(1)));
        assert!(esc.contains(&Local(0)));
    }
}

//! Escape analysis (DESIGN §7.2 Escape Analysis).
//!
//! Conservative: a local escapes if it may be observed after the current
//! function returns, stored into a heap object that escapes, passed to an
//! unknown callee, or read from a named `var` that escapes.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pass;
    use lumia_core::{Block, CoreFun, Op, Value};
    use lumia_hir::Builtin;
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
        let mut module = CoreModule::with_functions("M", vec![len_fun, main_fun]);
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

    #[test]
    fn short_lived_var_assign_does_not_escape() {
        // `var xs = listOf(1)` used only for a pure projection must not escape.
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
                    value: Value::AllocList {
                        elems: vec![Local(0)],
                        repr: lumia_core::ListRepr::HeapList,
                    },
                    pure_region: true,
                },
                Op::Assign {
                    name: "xs".into(),
                    value: Local(1),
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Builtin {
                        name: lumia_hir::Builtin::ListLen,
                        args: vec![Local(1)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(
            !esc.contains(&Local(1)),
            "non-escaping var list must stay stack-eligible: {esc:?}"
        );
    }

    #[test]
    fn var_name_read_that_returns_escapes_assigns() {
        let body = Block {
            params: vec![],
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::AllocList {
                        elems: vec![],
                        repr: lumia_core::ListRepr::HeapList,
                    },
                    pure_region: true,
                },
                Op::Assign {
                    name: "xs".into(),
                    value: Local(0),
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Name("xs".into()),
                    pure_region: true,
                },
            ],
            result: Some(Local(1)),
        };
        let esc = escaping_locals(&fun_with_body(body));
        assert!(
            esc.contains(&Local(0)),
            "returning Name(xs) must escape assigns to xs: {esc:?}"
        );
    }
}

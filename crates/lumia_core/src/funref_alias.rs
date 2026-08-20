//! FunRef alias environment: SSA locals + named slots (`var next = f`).
//!
//! `directize` already tracked both. Collect / lambda-lift / codegen TCO each
//! had a locals-only copy, so `Name` loads after `Assign` dropped the alias
//! (IndirectCall stayed indirect; TCO SCC missed the edge).

use crate::ir::{Block, Local, Op, Value};
use crate::visit::for_each_top_level_op_in_block;
use lumia_hir::Sym;
use rustc_hash::FxHashMap as HashMap;

/// Whether [`Value::AllocClosure`] should alias its lifted fun name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunRefAlloc {
    /// Cap-ty / lift: the closure local is the lifted fun.
    Track,
    /// Emit / TCO / directize of calls: only true [`Value::FunRef`] chains.
    Ignore,
}

/// Let-ordered FunRef aliases (definition order matters; not a DFS).
#[derive(Clone, Debug, Default)]
pub struct FunRefAliases {
    pub locals: HashMap<u32, Sym>,
    pub slots: HashMap<Sym, Sym>,
}

impl FunRefAliases {
    pub fn resolve(&self, local: u32) -> Option<&str> {
        self.locals.get(&local).map(Sym::as_str)
    }

    pub fn note_let(
        &mut self,
        local: u32,
        value: &Value,
        alloc: FunRefAlloc,
        cap_funs: Option<&HashMap<u32, Sym>>,
    ) {
        match value {
            Value::FunRef(name) => {
                self.locals.insert(local, name.name.clone());
            }
            Value::AllocClosure { fun, .. } if matches!(alloc, FunRefAlloc::Track) => {
                self.locals.insert(local, fun.name.clone());
            }
            Value::Local(Local(src)) => self.copy_local(local, *src),
            Value::Name(n) => {
                if let Some(fr) = self.slots.get(n).cloned() {
                    self.locals.insert(local, fr);
                } else {
                    self.locals.remove(&local);
                }
            }
            Value::ClosureCap { index, .. } => {
                if let Some(n) = cap_funs.and_then(|m| m.get(index)).cloned() {
                    self.locals.insert(local, n);
                } else {
                    self.locals.remove(&local);
                }
            }
            _ => {
                self.locals.remove(&local);
            }
        }
    }

    pub fn note_assign(&mut self, name: Sym, value: Local) {
        if let Some(fr) = self.locals.get(&value.0).cloned() {
            self.slots.insert(name, fr);
        } else {
            self.slots.remove(&name);
        }
    }

    fn copy_local(&mut self, dst: u32, src: u32) {
        if let Some(n) = self.locals.get(&src).cloned() {
            self.locals.insert(dst, n);
        } else {
            self.locals.remove(&dst);
        }
    }

    /// Visit each Let value with aliases as of *before* that Let, then nested
    /// If/Loop (inherited clone) / Lambda (fresh). Then note the Let / Assign.
    pub fn walk_block(
        &mut self,
        block: &Block,
        alloc: FunRefAlloc,
        cap_funs: Option<&HashMap<u32, Sym>>,
        on_value: &mut impl FnMut(&Value, &FunRefAliases),
    ) {
        for_each_top_level_op_in_block(block, &mut |op| match op {
            Op::Let { local, value, .. } => {
                on_value(value, self);
                match value {
                    Value::If {
                        then_block,
                        else_block,
                        ..
                    } => {
                        self.clone()
                            .walk_block(then_block, alloc, cap_funs, on_value);
                        self.clone()
                            .walk_block(else_block, alloc, cap_funs, on_value);
                    }
                    Value::Loop {
                        header,
                        body,
                        latch,
                    } => {
                        self.clone().walk_block(header, alloc, cap_funs, on_value);
                        self.clone().walk_block(body, alloc, cap_funs, on_value);
                        self.clone().walk_block(latch, alloc, cap_funs, on_value);
                    }
                    Value::Lambda { body, .. } => {
                        Self::default().walk_block(body, alloc, cap_funs, on_value);
                    }
                    _ => {}
                }
                self.note_let(local.0, value, alloc, cap_funs);
            }
            Op::Assign { name, value } => self.note_assign(name.clone(), *value),
            Op::Break | Op::Continue | Op::Return { .. } => {}
        });
    }
}

/// Direct callee names (`Call` + FunRef-resolved `IndirectCall`), slot aliases included.
pub fn collect_funref_callees(block: &Block, out: &mut impl Extend<Sym>) {
    FunRefAliases::default().walk_block(block, FunRefAlloc::Ignore, None, &mut |value, aliases| {
        match value {
            Value::Call { fun, .. } => out.extend(std::iter::once(fun.name.clone())),
            Value::IndirectCall { callee, .. } => {
                if let Some(n) = aliases.resolve(callee.0) {
                    out.extend(std::iter::once(Sym::from(n)));
                }
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Op;

    #[test]
    fn name_load_after_assign_resolves() {
        let mut a = FunRefAliases::default();
        a.note_let(0, &Value::FunRef("odd".into()), FunRefAlloc::Ignore, None);
        a.note_assign(Sym::from("next"), Local(0));
        a.note_let(1, &Value::Name("next".into()), FunRefAlloc::Ignore, None);
        assert_eq!(a.resolve(1), Some("odd"));
    }

    #[test]
    fn collect_callees_follows_slot_funref() {
        let block = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::FunRef("odd".into()),
                    pure_region: true,
                },
                Op::Assign {
                    name: "next".into(),
                    value: Local(0),
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Name("next".into()),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::IndirectCall {
                        callee: Local(1),
                        args: vec![],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        };
        let mut names = Vec::new();
        collect_funref_callees(&block, &mut names);
        assert_eq!(names, vec![Sym::from("odd")]);
    }
}

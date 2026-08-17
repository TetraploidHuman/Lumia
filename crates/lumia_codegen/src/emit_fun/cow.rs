//! COW reassignment detection (list/map set, ADT `with`).

use super::super::Codegen;
use lumia_core::{Block, Local, Op, Value};
use lumia_hir::Builtin;

impl<'ctx> Codegen<'ctx> {
    /// `Name`/`Local` alias or `AdtField` extract — not a fresh alloc / call result.
    pub(super) fn value_is_cow_alias(value: &Value) -> bool {
        matches!(
            value,
            Value::Local(_)
                | Value::Name(_)
                | Value::Builtin {
                    name: Builtin::AdtField,
                    ..
                }
        )
    }

    /// `xs = xs.set(…)` / `xs = xs.append(…)` — next op assigns this COW result
    /// back onto the loaded slot (unique RC can mutate in place).
    pub(super) fn cow_reassign_consumes(
        &self,
        block: &Block,
        let_idx: usize,
        dest: Local,
        value: &Value,
    ) -> bool {
        let Value::Builtin { name, args, .. } = value else {
            return false;
        };
        let list_arg = match name {
            lumia_hir::Builtin::MapSet | lumia_hir::Builtin::ListAppend => args.first(),
            lumia_hir::Builtin::ListConcat => args.first(),
            _ => None,
        };
        let Some(list_arg) = list_arg else {
            return false;
        };
        let Some(Value::Name(slot)) = self.frame.leaf_defs.get(&list_arg.0) else {
            return false;
        };
        matches!(
            block.ops.get(let_idx + 1),
            Some(Op::Assign { name, value: v }) if name == slot && *v == dest
        )
    }

    /// `p = p with { f = … }` lowered to unique/COW in-place field updates.
    ///
    /// Requires alias/`AdtField` retains (`bind_let_after_emit`) and
    /// `lumia_adt_ensure_unique_consume_mask` (drops the with-temp `Name(slot)`
    /// retain; overwrite mask skips nested retain on rewritten fields).
    pub(super) fn match_adt_with_reassign(
        &self,
        block: &Block,
        let_idx: usize,
        dest: Local,
        value: &Value,
    ) -> Option<(String, Vec<(u32, Local)>)> {
        let Value::AllocAdt { fields, .. } = value else {
            return None;
        };
        let Op::Assign {
            name: slot,
            value: v,
        } = block.ops.get(let_idx + 1)?
        else {
            return None;
        };
        if *v != dest {
            return None;
        }
        let mut updates = Vec::new();
        let mut saw_base = false;
        for (i, f) in fields.iter().enumerate() {
            match self.adt_field_from_slot(f, slot) {
                Some(idx) if idx as usize == i => {
                    saw_base = true;
                }
                Some(_) => return None, // wrong field index
                None => {
                    updates.push((i as u32, *f));
                }
            }
        }
        if !saw_base || updates.is_empty() {
            return None;
        }
        Some((slot.clone(), updates))
    }

    /// `AdtField(Name(slot)|alias, idx)` → Some(idx).
    fn adt_field_from_slot(&self, field: &Local, slot: &str) -> Option<i64> {
        let Value::Builtin {
            name: Builtin::AdtField,
            args, .. } = self.frame.leaf_defs.get(&field.0)?
        else {
            return None;
        };
        let base = args.first()?;
        let idx_l = args.get(1)?;
        let base_ok = match self.frame.leaf_defs.get(&base.0) {
            Some(Value::Name(n)) if n == slot => true,
            Some(Value::Local(Local(src))) => matches!(
                self.frame.leaf_defs.get(src),
                Some(Value::Name(n)) if n == slot
            ),
            _ => false,
        };
        if !base_ok {
            return None;
        }
        self.frame.local_int_consts.get(&idx_l.0).copied()
    }

}

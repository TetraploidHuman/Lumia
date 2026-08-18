//! COW reassignment detection (list/map set, ADT `with`).

use super::super::Codegen;
use anyhow::Result;
use inkwell::values::IntValue;
use lumia_core::{Block, Local, Op, Value};
use lumia_hir::Builtin;

/// Builtins whose emit retains the source container for COW when the old
/// binding stays live (`!cow_consume_unique`).
pub(crate) fn is_cow_container_mutator(b: Builtin) -> bool {
    matches!(
        b,
        Builtin::MapSet
            | Builtin::ListAppend
            | Builtin::ListConcat
            | Builtin::SetInsert
            | Builtin::MapRemove
            | Builtin::ListReverse
            | Builtin::ListSort
            | Builtin::ListSortByKeys
    )
}

impl<'ctx> Codegen<'ctx> {
    /// Retain container (+ optional nested payload) for
    /// [`is_cow_container_mutator`] builtins.
    ///
    /// Container retain is skipped for proven `xs = xs.set/append/concat(…)`;
    /// payload retain keeps nested COW safe when the container is aliased.
    pub(crate) fn cow_retain_mutator_args(
        &mut self,
        container_i64: IntValue<'ctx>,
        payload: Option<(Local, IntValue<'ctx>)>,
    ) -> Result<()> {
        if !self.frame.cow_consume_unique {
            self.list_retain_i64(container_i64)?;
        }
        if let Some((local, bits)) = payload {
            if let Some(ty) = self.frame.local_tys.get(&local.0) {
                if Self::type_needs_cow_retain(ty) {
                    self.adt_retain_i64(bits)?;
                }
            }
        }
        Ok(())
    }

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
        if !is_cow_container_mutator(*name) {
            return false;
        }
        let Some(list_arg) = args.first() else {
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
            args,
            ..
        } = self.frame.leaf_defs.get(&field.0)?
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

#[cfg(test)]
mod tests {
    use super::is_cow_container_mutator;
    use lumia_hir::Builtin;

    #[test]
    fn cow_container_mutator_set_is_mapset_append_concat_insert() {
        assert!(is_cow_container_mutator(Builtin::MapSet));
        assert!(is_cow_container_mutator(Builtin::ListAppend));
        assert!(is_cow_container_mutator(Builtin::ListConcat));
        assert!(is_cow_container_mutator(Builtin::SetInsert));
        assert!(is_cow_container_mutator(Builtin::MapRemove));
        assert!(is_cow_container_mutator(Builtin::ListReverse));
        assert!(is_cow_container_mutator(Builtin::ListSort));
        assert!(is_cow_container_mutator(Builtin::ListSortByKeys));
        assert!(!is_cow_container_mutator(Builtin::ListTake));
        assert!(!is_cow_container_mutator(Builtin::StrSubstring));
    }
}

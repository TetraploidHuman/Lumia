//! Let binding, ephemeral root elision, and local-use walks.

use super::super::Codegen;
use anyhow::Result;
use inkwell::values::BasicValueEnum;
use lumia_core::{Block, Local, Op, Value};
use lumia_hir::Builtin;
use lumia_ty::Type;

impl<'ctx> Codegen<'ctx> {
    /// Best-effort expected type for empty container literals (Float tags).
    pub(super) fn peek_expected_alloc_ty(
        &self,
        block: &Block,
        idx: usize,
        local: Local,
        value: &Value,
    ) -> Option<Type> {
        let empty = match value {
            Value::AllocList { elems, .. } => elems.is_empty(),
            Value::AllocSet { elems, .. } => elems.is_empty(),
            Value::AllocMap { flat_pairs, .. } => flat_pairs.is_empty(),
            _ => return None,
        };
        if !empty {
            return None;
        }
        if let Some(Op::Assign { name, value: v }) = block.ops.get(idx + 1) {
            if *v == local {
                if let Some(ty) = self.frame.slot_tys.get(name) {
                    return Some(ty.clone());
                }
            }
        }
        if block.result == Some(local) {
            return self.funs.fun_ret_tys.get(&self.funs.current_fun).cloned();
        }
        None
    }

    pub(super) fn bind_let_after_emit(
        &mut self,
        local: Local,
        value: &Value,
        v: BasicValueEnum<'ctx>,
    ) -> Result<()> {
        let mut ty = self.infer_value_ty(value);
        // Per-channel send agreement: override `Channel(Int)` placeholder.
        if matches!(
            value,
            Value::Builtin {
                name: lumia_hir::Builtin::ChannelNew,
                ..
            }
        ) {
            if let Some(elem) = self.funs.channel_elem_by_local.get(&local.0) {
                ty = Type::Channel(Box::new(elem.clone()));
            }
        }
        // Captures keep the AllocClosure-site type (Channel[Float], Fun, …).
        if let Value::ClosureCap { index, as_float, .. } = value {
            if *as_float {
                ty = Type::Float;
            } else if let Some(cap_ty) = self
                .funs
                .closure_cap_tys
                .get(&self.funs.current_fun)
                .and_then(|m| m.get(index))
                .cloned()
            {
                ty = cap_ty;
            }
        }
        if let Ok(bits) = self.coerce_i64(v) {
            // `Name`/`Local` are not `value_alloc_may_heap`, but aliases of List/ADT
            // still need COW retain + GC roots (`val snap = p`).
            if Self::type_needs_cow_retain(&ty) && Self::value_is_cow_alias(value) {
                self.adt_retain_i64(bits)?;
            }
            if self.value_may_heap(value) || Self::type_may_heap(&ty) {
                self.root_push_i64(bits)?;
            }
        }
        self.frame.locals.insert(local.0, v);
        self.frame.local_tys.insert(local.0, ty);
        self.note_int_const(local.0, value);
        crate::funref::note_funref_local(
            &mut self.funs.funref_locals,
            local.0,
            value,
            crate::funref::AllocClosureFunref::Ignore,
        );
        Ok(())
    }

    /// Track `Value::Int` / aliases so `AdtField` can resolve `params[idx]`.
    pub(super) fn note_int_const(&mut self, local: u32, value: &Value) {
        match value {
            Value::Int(n) => {
                self.frame.local_int_consts.insert(local, *n);
            }
            Value::Local(Local(src)) => {
                if let Some(n) = self.frame.local_int_consts.get(src).copied() {
                    self.frame.local_int_consts.insert(local, n);
                } else {
                    self.frame.local_int_consts.remove(&local);
                }
            }
            _ => {
                self.frame.local_int_consts.remove(&local);
            }
        }
    }

    /// `slot = <heap expr>` lowered as `Let t = expr; Assign slot := t` with no other uses of `t`.
    /// The mut slot is already a GC root, so the temp need not be shadow-stack rooted.
    pub(super) fn let_only_feeds_next_assign(&self, block: &Block, let_idx: usize, local: Local) -> bool {
        let Some(Op::Assign { value, .. }) = block.ops.get(let_idx + 1) else {
            return false;
        };
        if *value != local {
            return false;
        }
        if block.result == Some(local) {
            return false;
        }
        for op in &block.ops[let_idx + 2..] {
            if Self::op_uses_local(op, local) {
                return false;
            }
        }
        true
    }

    /// `Let t = Name(xs)|Local(…)` where `t` is only the receiver of list/map
    /// get/set/append/concat/len later in this block — the source is already rooted
    /// (mut slot or prior let), so skip retain+root on the alias.
    pub(super) fn let_is_ephemeral_rooted_recv(
        &self,
        block: &Block,
        let_idx: usize,
        local: Local,
        value: &Value,
    ) -> bool {
        if !matches!(value, Value::Name(_) | Value::Local(_)) {
            return false;
        }
        let ty = self.infer_value_ty(value);
        if !matches!(ty, Type::List(_) | Type::Map(_, _) | Type::Set(_)) {
            return false;
        }
        if block.result == Some(local) {
            return false;
        }
        let mut uses = 0usize;
        let mut only_recv = true;
        for op in &block.ops[let_idx + 1..] {
            if !Self::op_uses_local(op, local) {
                continue;
            }
            uses += 1;
            let ok = match op {
                Op::Let {
                    value: Value::Builtin { name, args, .. },
                    ..
                } => {
                    let recv = matches!(
                        name,
                        lumia_hir::Builtin::ListGet
                            | lumia_hir::Builtin::ListLen
                            | lumia_hir::Builtin::ListAppend
                            | lumia_hir::Builtin::ListConcat
                            | lumia_hir::Builtin::ListTake
                            | lumia_hir::Builtin::ListSlice
                            | lumia_hir::Builtin::MapSet
                            | lumia_hir::Builtin::MapRemove
                            | lumia_hir::Builtin::Contains
                    );
                    recv && args.first() == Some(&local) && args[1..].iter().all(|a| *a != local)
                }
                _ => false,
            };
            if !ok {
                only_recv = false;
                break;
            }
        }
        uses >= 1 && only_recv
    }

    /// `Let t = Name/Local` used only as a `Call`/`IndirectCall` argument.
    ///
    /// The source is already live/rooted; a temporary retain would bump COW RC and
    /// force `ensure_unique` clones inside `lumia_cn_*` / `lumia_f64_*` kernels.
    ///
    /// **Not** applied to `AdtField`: extracting a List/heap field without retain
    /// lets the parent ADT drop while the callee still holds the unreained
    /// pointer (`makeObs` → `nearest(eco, eco.ecoThreats, n)` zeroed threat obs).
    pub(super) fn let_is_ephemeral_call_arg(
        &self,
        block: &Block,
        let_idx: usize,
        local: Local,
        value: &Value,
    ) -> bool {
        let is_alias = matches!(value, Value::Name(_) | Value::Local(_));
        if !is_alias {
            return false;
        }
        let ty = self.infer_value_ty(value);
        if !Self::type_needs_cow_retain(&ty) {
            return false;
        }
        if block.result == Some(local) {
            return false;
        }
        let mut uses = 0usize;
        let mut only_arg = true;
        for op in &block.ops[let_idx + 1..] {
            if !Self::op_uses_local(op, local) {
                continue;
            }
            uses += 1;
            let ok = match op {
                Op::Let { value, .. } => match value {
                    Value::Call { args, .. } | Value::IndirectCall { args, .. } => {
                        args.contains(&local)
                    }
                    _ => false,
                },
                _ => false,
            };
            if !ok {
                only_arg = false;
                break;
            }
        }
        uses >= 1 && only_arg
    }

    /// `Let t = AdtField(…)` used only as the receiver of further `AdtField`
    /// extracts (e.g. temps for `o.cell.n`). Read-only nested chains are safe:
    /// the parent stays rooted and `t` is never a `with` receiver.
    ///
    /// **Not** applied to `Name`/`Local` (`load p` / `val snap = p`): those
    /// retains feed COW — both snapshots and the with-temp that
    /// `ensure_unique_consume` drops (`examples/adt_with_alias.lm`).
    pub(super) fn let_is_ephemeral_adt_field_base(
        &self,
        block: &Block,
        let_idx: usize,
        local: Local,
        value: &Value,
    ) -> bool {
        let Value::Builtin {
            name: Builtin::AdtField,
            ..
        } = value
        else {
            return false;
        };
        let ty = self.infer_value_ty(value);
        if !matches!(ty, Type::Adt { .. }) {
            return false;
        }
        if block.result == Some(local) {
            return false;
        }
        let mut uses = 0usize;
        let mut only_base = true;
        for op in &block.ops[let_idx + 1..] {
            if !Self::op_uses_local(op, local) {
                continue;
            }
            uses += 1;
            let ok = match op {
                Op::Let { value, .. } => match value {
                    Value::Builtin {
                        name: Builtin::AdtField,
                        args, .. } => args.first() == Some(&local) && args[1..].iter().all(|a| *a != local),
                    _ => false,
                },
                _ => false,
            };
            if !ok {
                only_base = false;
                break;
            }
        }
        uses >= 1 && only_base
    }

    /// `Let t = AdtField(base,…)` whose only use is an unchanged field of an
    /// `AllocAdt` rewritten to inplace `slot = slot with {…}` — skip retain+root
    /// (codegen ignores these extracts on the inplace path).
    pub(super) fn let_is_unused_inplace_with_field(
        &self,
        block: &Block,
        let_idx: usize,
        local: Local,
        value: &Value,
    ) -> bool {
        let Value::Builtin {
            name: Builtin::AdtField,
            ..
        } = value
        else {
            return false;
        };
        if block.result == Some(local) {
            return false;
        }
        let mut uses = 0usize;
        let mut only_dead_with_field = true;
        for (op_i, op) in block.ops[let_idx + 1..].iter().enumerate() {
            if !Self::op_uses_local(op, local) {
                continue;
            }
            uses += 1;
            let abs_i = let_idx + 1 + op_i;
            let ok = match op {
                Op::Let {
                    local: dest,
                    value: alloc @ Value::AllocAdt { fields, .. },
                    ..
                } => match self.match_adt_with_reassign(block, abs_i, *dest, alloc) {
                    Some((_slot, updates)) => {
                        fields.contains(&local) && updates.iter().all(|(_, u)| *u != local)
                    }
                    None => false,
                },
                _ => false,
            };
            if !ok {
                only_dead_with_field = false;
                break;
            }
        }
        uses >= 1 && only_dead_with_field
    }

    fn op_uses_local(op: &Op, local: Local) -> bool {
        match op {
            Op::Let { value, .. } => Self::value_uses_local(value, local),
            Op::Assign { value, .. } | Op::Return { value } => *value == local,
            Op::Break | Op::Continue => false,
        }
    }

    fn value_uses_local(value: &Value, local: Local) -> bool {
        let mut hit = false;
        lumia_core::for_each_local(value, &mut |l| {
            if l == local {
                hit = true;
            }
        });
        if hit {
            return true;
        }
        let mut nested_hit = false;
        lumia_core::for_each_nested_block(value, &mut |b| {
            if Self::block_uses_local(b, local) {
                nested_hit = true;
            }
        });
        nested_hit
    }

    fn block_uses_local(block: &Block, local: Local) -> bool {
        if block.result == Some(local) {
            return true;
        }
        block.ops.iter().any(|op| Self::op_uses_local(op, local))
    }
}

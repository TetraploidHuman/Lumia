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
        if let Value::ClosureCap { index, .. } = value {
            if let Some(cap_ty) = self
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

    /// Like [`Self::bind_let_after_emit`] but omit `root_push` when the live
    /// range has no safepoint (see [`Self::let_skip_root_no_safepoint`]).
    pub(super) fn bind_let_skip_root(
        &mut self,
        local: Local,
        value: &Value,
        v: BasicValueEnum<'ctx>,
    ) -> Result<()> {
        let mut ty = self.infer_value_ty(value);
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
        if let Value::ClosureCap { index, .. } = value {
            if let Some(cap_ty) = self
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
            if Self::type_needs_cow_retain(&ty) && Self::value_is_cow_alias(value) {
                self.adt_retain_i64(bits)?;
            }
            // Intentionally no root_push — live range proven safepoint-free.
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
    pub(super) fn let_only_feeds_next_assign(
        &self,
        block: &Block,
        let_idx: usize,
        local: Local,
    ) -> bool {
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

    /// `Let t = Name(xs)|Local(…)|fresh list` where `t` is only the receiver of
    /// list/map get/set/append/… later in this block.
    ///
    /// Alias forms are already rooted (mut slot / prior let). Fresh
    /// `AllocList` / `range` / empty list may skip root only for non-allocating
    /// receivers (`get`/`len`/`contains`) — append/concat can GC.
    pub(super) fn let_is_ephemeral_rooted_recv(
        &self,
        block: &Block,
        let_idx: usize,
        local: Local,
        value: &Value,
    ) -> bool {
        let already_rooted = matches!(value, Value::Name(_) | Value::Local(_));
        let fresh_list = matches!(
            value,
            Value::AllocList { .. }
                | Value::Builtin {
                    name: Builtin::Range | Builtin::RangeInclusive,
                    ..
                }
        );
        if !already_rooted && !fresh_list {
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
                    let read_only = matches!(
                        name,
                        Builtin::ListGet | Builtin::ListLen | Builtin::Contains
                    );
                    let mutating = matches!(
                        name,
                        Builtin::ListAppend
                            | Builtin::ListConcat
                            | Builtin::ListTake
                            | Builtin::ListSlice
                            | Builtin::MapSet
                            | Builtin::MapRemove
                            | Builtin::SetInsert
                            | Builtin::ListReverse
                            | Builtin::ListSort
                            | Builtin::ListSortByKeys
                    );
                    let recv_ok = if already_rooted {
                        read_only || mutating
                    } else {
                        // Fresh producer: only ops that must not GC before `t` is dead.
                        read_only
                    };
                    recv_ok && args.first() == Some(&local) && args[1..].iter().all(|a| *a != local)
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
                        args,
                        ..
                    } => args.first() == Some(&local) && args[1..].iter().all(|a| *a != local),
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

    /// Heap let whose last-use interval (including nested **`If` arms** and
    /// **`Loop` regions**) contains no safepoint — skip `root_push` (retain
    /// still applied in [`Self::bind_let_skip_root`]).
    ///
    /// - Allows uses inside pure `If` and GC-free `Loop` (header/body/latch scanned).
    /// - Refuses uses under `Lambda` (heap closure capture).
    /// - Refuses `block.result` (live until scope end).
    /// - A `Lambda` **between** bind and last use still counts as a safepoint;
    ///   a `Loop` only if any nested op may GC.
    pub(super) fn let_skip_root_no_safepoint(
        &self,
        block: &Block,
        let_idx: usize,
        local: Local,
        value: &Value,
    ) -> bool {
        let ty = self.infer_value_ty(value);
        if !(self.value_may_heap(value) || Self::type_may_heap(&ty)) {
            return false;
        }
        if block.result == Some(local) {
            return false;
        }
        live_range_skip_root_ok(block, let_idx, local)
    }

    /// Record a just-pushed SSA/param root for last-use early `root_pop`.
    ///
    /// Skips `block.result` (live until scope end). Nested blocks pass
    /// [`Self::pop_dead_ssa_roots`] a stack base so they cannot pop outer roots.
    pub(super) fn note_ssa_root(&mut self, block: &Block, after: usize, local: Local) {
        if block.result == Some(local) {
            return;
        }
        let last_use = last_use_index(block, after, local);
        self.frame.ssa_root_stack.push(crate::state::SsaRoot {
            last_use,
            depth: self.frame.root_depth,
        });
    }

    /// Pop trailing unused SSA roots (`last_use == None`) that are the shadow-stack top.
    pub(super) fn pop_unused_ssa_roots(&mut self, stack_base: usize) -> Result<()> {
        while self.frame.ssa_root_stack.len() > stack_base {
            let Some(top) = self.frame.ssa_root_stack.last() else {
                break;
            };
            if top.last_use.is_some() || self.frame.root_depth != top.depth {
                break;
            }
            let new_depth = top.depth.saturating_sub(1);
            self.root_pop_to(new_depth)?;
        }
        Ok(())
    }

    /// Pop trailing dead SSA roots that are the current shadow-stack top.
    pub(super) fn pop_dead_ssa_roots(&mut self, idx: usize, stack_base: usize) -> Result<()> {
        while self.frame.ssa_root_stack.len() > stack_base {
            let Some(top) = self.frame.ssa_root_stack.last() else {
                break;
            };
            if top.last_use.is_some_and(|lu| lu > idx) || self.frame.root_depth != top.depth {
                break;
            }
            let new_depth = top.depth.saturating_sub(1);
            self.root_pop_to(new_depth)?;
        }
        Ok(())
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

/// Last op index in `block.ops[after..]` that uses `local` (nested If/Loop count
/// as a use of the containing op). `None` if unused after `after`.
pub(crate) fn last_use_index(block: &Block, after: usize, local: Local) -> Option<usize> {
    let mut last = None;
    for (i, op) in block.ops.iter().enumerate().skip(after) {
        if op_uses_local(op, local) {
            last = Some(i);
        }
    }
    last
}

/// Live-range half of skip-root (no heap / `block.result` checks).
///
/// `true` ⇒ interval after `let_idx` until last use of `local` has no safepoint
/// and `local` is not used under `Lambda`.
pub(crate) fn live_range_skip_root_ok(block: &Block, let_idx: usize, local: Local) -> bool {
    if local_used_under_lambda(block, let_idx, local) {
        return false;
    }
    let Some(last) = last_use_index(block, let_idx + 1, local) else {
        return true;
    };
    for op in &block.ops[let_idx + 1..=last] {
        if op_may_safepoint(op) {
            return false;
        }
    }
    true
}

/// Simulated LIFO early-pop: after emitting op `idx`, drop trailing SSA roots
/// whose last use is `None` (already dead) or `<= idx`, while they are the
/// shadow-stack top.
///
/// `stack_base` hides outer-block roots (nested If/Loop must not pop them).
pub(crate) fn pop_dead_ssa_roots_sim(
    stack: &mut Vec<(Option<usize>, u32)>,
    root_depth: &mut u32,
    idx: usize,
    stack_base: usize,
) {
    while stack.len() > stack_base {
        let Some(&(last_use, depth)) = stack.last() else {
            break;
        };
        if last_use.is_some_and(|lu| lu > idx) {
            break;
        }
        if *root_depth != depth {
            break;
        }
        *root_depth -= 1;
        stack.pop();
    }
}

fn local_used_under_lambda(block: &Block, let_idx: usize, local: Local) -> bool {
    for op in &block.ops[let_idx + 1..] {
        if let Op::Let { value, .. } = op {
            if value_uses_local_under_lambda(value, local) {
                return true;
            }
        }
    }
    false
}

fn value_uses_local_under_lambda(value: &Value, local: Local) -> bool {
    match value {
        Value::Lambda { body, .. } => block_uses_local(body, local),
        Value::Loop {
            header,
            body,
            latch,
        } => {
            block_uses_local_under_lambda(header, local)
                || block_uses_local_under_lambda(body, local)
                || block_uses_local_under_lambda(latch, local)
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            block_uses_local_under_lambda(then_block, local)
                || block_uses_local_under_lambda(else_block, local)
        }
        _ => false,
    }
}

fn block_uses_local_under_lambda(block: &Block, local: Local) -> bool {
    for op in &block.ops {
        if let Op::Let { value, .. } = op {
            if value_uses_local_under_lambda(value, local) {
                return true;
            }
        }
    }
    false
}

fn op_may_safepoint(op: &Op) -> bool {
    match op {
        Op::Let { value, .. } => value_may_safepoint(value),
        Op::Assign { .. } | Op::Return { .. } | Op::Break | Op::Continue => false,
    }
}

fn block_may_safepoint(block: &Block) -> bool {
    block.ops.iter().any(op_may_safepoint)
}

fn value_may_safepoint(value: &Value) -> bool {
    match value {
        Value::Lambda { .. } => true,
        Value::Loop {
            header,
            body,
            latch,
        } => block_may_safepoint(header) || block_may_safepoint(body) || block_may_safepoint(latch),
        Value::If {
            then_block,
            else_block,
            ..
        } => block_may_safepoint(then_block) || block_may_safepoint(else_block),
        Value::Call { .. } | Value::IndirectCall { .. } => true,
        Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. } => true,
        Value::Builtin { name, .. } => !matches!(
            name,
            Builtin::ListGet
                | Builtin::ListLen
                | Builtin::Contains
                | Builtin::AdtField
                | Builtin::AdtTag
        ),
        Value::Binary { .. }
        | Value::Unary { .. }
        | Value::Name(_)
        | Value::Local(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::Char(_)
        | Value::Unit
        | Value::String(_)
        | Value::FunRef(_)
        | Value::ClosureCap { .. } => false,
    }
}

fn op_uses_local(op: &Op, local: Local) -> bool {
    match op {
        Op::Let { value, .. } => value_uses_local(value, local),
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
        if block_uses_local(b, local) {
            nested_hit = true;
        }
    });
    nested_hit
}

fn block_uses_local(block: &Block, local: Local) -> bool {
    if block.result == Some(local) {
        return true;
    }
    block.ops.iter().any(|op| op_uses_local(op, local))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_core::{Block, ListRepr};

    fn empty_block(result: Option<Local>) -> Block {
        Block {
            ops: vec![],
            result,
        }
    }

    fn let_op(local: Local, value: Value) -> Op {
        Op::Let {
            local,
            value,
            pure_region: false,
        }
    }

    fn alloc_list(elems: Vec<Local>) -> Value {
        Value::AllocList {
            elems,
            repr: ListRepr::HeapList,
        }
    }

    fn list_len(xs: Local) -> Value {
        Value::Builtin {
            name: Builtin::ListLen,
            args: vec![xs],
            result_ty: None,
        }
    }

    #[test]
    fn skip_root_allows_use_inside_pure_if() {
        let xs = Local(0);
        let cond = Local(1);
        let len = Local(2);
        let then_b = Block {
            ops: vec![let_op(len, list_len(xs))],
            result: Some(len),
        };
        let else_b = empty_block(Some(Local(3)));
        let n = Local(4);
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(
                    n,
                    Value::If {
                        cond,
                        then_block: Box::new(then_b),
                        else_block: Box::new(else_b),
                    },
                ),
            ],
            result: Some(n),
        };
        assert!(
            live_range_skip_root_ok(&block, 0, xs),
            "pure If + ListLen must allow skip-root"
        );
    }

    #[test]
    fn skip_root_refuses_call_inside_if_arm() {
        let xs = Local(0);
        let cond = Local(1);
        let then_b = Block {
            ops: vec![let_op(
                Local(2),
                Value::Call {
                    fun: "println".into(),
                    args: vec![xs],
                },
            )],
            result: None,
        };
        let else_b = empty_block(None);
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(
                    Local(3),
                    Value::If {
                        cond,
                        then_block: Box::new(then_b),
                        else_block: Box::new(else_b),
                    },
                ),
            ],
            result: None,
        };
        assert!(
            !live_range_skip_root_ok(&block, 0, xs),
            "Call in If arm is a safepoint"
        );
    }

    fn empty_loop() -> Value {
        Value::Loop {
            header: Box::new(empty_block(None)),
            body: Box::new(empty_block(None)),
            latch: Box::new(empty_block(None)),
        }
    }

    #[test]
    fn skip_root_allows_use_inside_gc_free_loop() {
        let xs = Local(0);
        let body = Block {
            ops: vec![let_op(Local(1), list_len(xs))],
            result: None,
        };
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(
                    Local(2),
                    Value::Loop {
                        header: Box::new(empty_block(None)),
                        body: Box::new(body),
                        latch: Box::new(empty_block(None)),
                    },
                ),
            ],
            result: None,
        };
        assert!(
            live_range_skip_root_ok(&block, 0, xs),
            "ListLen inside a GC-free Loop must allow skip-root"
        );
    }

    #[test]
    fn skip_root_refuses_call_inside_loop() {
        let xs = Local(0);
        let body = Block {
            ops: vec![let_op(
                Local(1),
                Value::Call {
                    fun: "println".into(),
                    args: vec![xs],
                },
            )],
            result: None,
        };
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(
                    Local(2),
                    Value::Loop {
                        header: Box::new(empty_block(None)),
                        body: Box::new(body),
                        latch: Box::new(empty_block(None)),
                    },
                ),
            ],
            result: None,
        };
        assert!(
            !live_range_skip_root_ok(&block, 0, xs),
            "Call in Loop body is a safepoint"
        );
    }

    #[test]
    fn skip_root_refuses_use_inside_lambda() {
        let xs = Local(0);
        let body = Block {
            ops: vec![let_op(Local(1), list_len(xs))],
            result: Some(Local(1)),
        };
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(
                    Local(2),
                    Value::Lambda {
                        params: vec![],
                        body: Box::new(body),
                    },
                ),
            ],
            result: None,
        };
        assert!(
            !live_range_skip_root_ok(&block, 0, xs),
            "use under Lambda (heap capture) must stay rooted"
        );
    }

    #[test]
    fn skip_root_allows_pure_if_between_bind_and_flat_use() {
        let xs = Local(0);
        let cond = Local(1);
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(
                    Local(2),
                    Value::If {
                        cond,
                        then_block: Box::new(empty_block(Some(Local(3)))),
                        else_block: Box::new(empty_block(Some(Local(4)))),
                    },
                ),
                let_op(Local(5), list_len(xs)),
            ],
            result: Some(Local(5)),
        };
        assert!(
            live_range_skip_root_ok(&block, 0, xs),
            "pure If between bind and flat use must allow skip-root"
        );
    }

    #[test]
    fn skip_root_allows_pure_loop_between_bind_and_flat_use() {
        let xs = Local(0);
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(Local(1), empty_loop()),
                let_op(Local(2), list_len(xs)),
            ],
            result: Some(Local(2)),
        };
        assert!(
            live_range_skip_root_ok(&block, 0, xs),
            "GC-free Loop between bind and ListLen must allow skip-root"
        );
    }

    #[test]
    fn last_use_index_sees_nested_if() {
        let xs = Local(0);
        let cond = Local(1);
        let then_b = Block {
            ops: vec![let_op(Local(2), list_len(xs))],
            result: Some(Local(2)),
        };
        let block = Block {
            ops: vec![
                let_op(xs, alloc_list(vec![])),
                let_op(
                    Local(3),
                    Value::If {
                        cond,
                        then_block: Box::new(then_b),
                        else_block: Box::new(empty_block(Some(Local(4)))),
                    },
                ),
                let_op(
                    Local(5),
                    Value::Call {
                        fun: "println".into(),
                        args: vec![],
                    },
                ),
            ],
            result: Some(Local(3)),
        };
        assert_eq!(last_use_index(&block, 1, xs), Some(1));
    }

    #[test]
    fn early_pop_lifo_pops_top_then_uncovers_dead() {
        // xs last_use=1, ys last_use=2; after 1 cannot pop xs (ys on top);
        // after 2 pop ys then xs.
        let mut stack = vec![(Some(1), 1u32), (Some(2), 2u32)];
        let mut depth = 2u32;
        pop_dead_ssa_roots_sim(&mut stack, &mut depth, 1, 0);
        assert_eq!(stack, vec![(Some(1), 1), (Some(2), 2)]);
        assert_eq!(depth, 2);
        pop_dead_ssa_roots_sim(&mut stack, &mut depth, 2, 0);
        assert!(stack.is_empty());
        assert_eq!(depth, 0);
    }

    #[test]
    fn early_pop_respects_nested_stack_base() {
        let mut stack = vec![(Some(0), 1u32), (Some(0), 2u32)];
        let mut depth = 2u32;
        pop_dead_ssa_roots_sim(&mut stack, &mut depth, 0, 1);
        assert_eq!(stack, vec![(Some(0), 1)]);
        assert_eq!(depth, 1);
    }

    #[test]
    fn early_pop_unused_none_is_immediately_dead() {
        let mut stack = vec![(None, 1u32)];
        let mut depth = 1u32;
        pop_dead_ssa_roots_sim(&mut stack, &mut depth, 0, 0);
        assert!(stack.is_empty());
        assert_eq!(depth, 0);
    }
}

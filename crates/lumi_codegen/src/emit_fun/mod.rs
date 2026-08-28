//! Function / block emission and mutable slots.

mod abi;
mod helpers;
mod slots;

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue};
#[cfg(feature = "opt-memo")]
use lumi_core::MemoTf;
use lumi_core::{Block, CoreFun, Local, Op, Value};
use lumi_hir::Builtin;
use lumi_ty::Type;

impl<'ctx> Codegen<'ctx> {
    pub(crate) fn emit_function(&mut self, fun: &CoreFun) -> Result<()> {
        let fv = *self
            .funs
            .functions
            .get(&fun.name)
            .context("missing function decl")?;
        let entry = self.llvm.context.append_basic_block(fv, "entry");
        self.llvm.builder.position_at_end(entry);
        self.frame.entry_bb = Some(entry);
        self.frame.locals.clear();
        self.frame.slots.clear();
        self.frame.float_slots.clear();
        self.frame.loop_stack.clear();
        self.memo.memo_arg_slots.clear();
        self.memo.memo_idx_key = None;
        self.frame.root_depth = 0;
        self.frame.rooted_slots.clear();
        self.frame.cow_consume_unique = false;
        self.frame.adt_with_inplace = None;
        self.funs.funref_locals.clear();
        self.frame.local_tys.clear();
        self.frame.local_int_consts.clear();
        self.frame.slot_tys.clear();
        self.frame.emit_dest = None;
        self.frame.expect_alloc_ty = None;
        self.frame.nsw_binop_locals = if self.nsw_iv {
            crate::nsw_iv::collect_nsw_binop_locals(&fun.body)
        } else {
            Default::default()
        };
        self.frame.safe_divisor_locals = crate::nsw_iv::collect_safe_divisor_locals(&fun.body);
        self.frame.nonneg_iv_load_locals = crate::nsw_iv::collect_nonneg_iv_load_locals(&fun.body);
        self.frame.leaf_defs = crate::nsw_iv::collect_leaf_defs(&fun.body);
        self.funs.current_fun = fun.name.clone();
        self.memo.current_memo = fun.memo;
        self.funs.tco_peers = self
            .funs
            .tco_sccs
            .get(&fun.name)
            .cloned()
            .unwrap_or_default();

        // Dense List[Float] helpers → thin RT trampoline (no frame / root traffic).
        if fun.memo.is_none() {
            for (i, p) in fun.params.iter().enumerate() {
                let av = fv.get_nth_param(i as u32).context("function param")?;
                let ty = fun.param_tys.get(i).cloned().unwrap_or(Type::Int);
                self.frame.local_tys.insert(p.0, ty.clone());
                if matches!(ty, Type::Float) {
                    let bits = av.into_int_value();
                    let f = crate::error::llvm(self.llvm.builder.build_bit_cast(
                        bits,
                        self.llvm.context.f64_type(),
                        "arg_f64",
                    ))?;
                    self.frame.locals.insert(p.0, f);
                } else {
                    self.frame.locals.insert(p.0, av);
                }
            }
            #[cfg(feature = "opt-dense-f64")]
            if self.try_emit_dense_f64_fun(fun, fv)?.is_some() {
                return Ok(());
            }
            // Fall through: clear param bindings; normal path re-binds with roots.
            self.frame.locals.clear();
            self.frame.local_tys.clear();
            self.frame.local_int_consts.clear();
        }

        let frame_name = if fun.is_main {
            "main"
        } else {
            fun.name.as_str()
        };
        self.emit_frame_push(frame_name)?;

        for (i, p) in fun.params.iter().enumerate() {
            let av = fv.get_nth_param(i as u32).context("function param")?;
            let ty = fun.param_tys.get(i).cloned().unwrap_or(Type::Int);
            self.frame.local_tys.insert(p.0, ty.clone());
            if matches!(ty, Type::Float) {
                let bits = av.into_int_value();
                let f = crate::error::llvm(self.llvm.builder.build_bit_cast(
                    bits,
                    self.llvm.context.f64_type(),
                    "arg_f64",
                ))?;
                self.frame.locals.insert(p.0, f);
            } else {
                self.frame.locals.insert(p.0, av);
                if Self::type_may_heap(&ty) {
                    let bits = self.coerce_i64(av)?;
                    self.root_push_i64(bits)?;
                }
            }
        }

        #[cfg(feature = "opt-memo")]
        {
            let compute_bb = match fun.memo {
                Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_prologue(fun, fv, id)?,
                Some(MemoTf::Slots { id }) => self.emit_memo_tf_prologue(fun, fv, id)?,
                None => entry,
            };
            if fun.memo.is_some() {
                self.llvm.builder.position_at_end(compute_bb);
            }
        }

        let result = self.emit_block(&fun.body, fv)?;
        // Tail-call / break paths may already have terminated the block.
        if self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some()
        {
            return Ok(());
        }
        let ret = result.unwrap_or_else(|| self.llvm.i64_ty.const_int(0, false).into());
        let ret_i = if matches!(fun.ret_ty, Type::Float) {
            match ret {
                BasicValueEnum::FloatValue(f) => crate::error::llvm(
                    self.llvm
                        .builder
                        .build_bit_cast(f, self.llvm.i64_ty, "ret_f64_bits"),
                )?
                .into_int_value(),
                other => self.coerce_i64(other)?,
            }
        } else {
            self.coerce_i64(ret)?
        };

        #[cfg(feature = "opt-memo")]
        match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_store(id, ret_i)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_tf_store(id, ret_i)?,
            None => {}
        }
        self.emit_return_i64(ret_i)?;
        Ok(())
    }

    pub(crate) fn infer_value_ty(&self, value: &Value) -> Type {
        let mut call_ret = |fun: &str, _args: &[Local]| self.funs.fun_ret_tys.get(fun).cloned();
        lumi_core::infer_value_ty_ctx(value, self.infer_ctx(), Some(&mut call_ret))
    }

    /// Best-effort expected type for empty container literals (Float tags).
    fn peek_expected_alloc_ty(
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

    /// Pure self/mutual recursion in tail position → musttail (DESIGN §4.4).
    /// Returns `Ok(true)` if the block was terminated by a musttail call.
    fn try_emit_tco_let(&mut self, block: &Block, local: Local, value: &Value) -> Result<bool> {
        let is_block_tail = block.result == Some(local)
            && matches!(
                block.ops.last(),
                Some(Op::Let { local: last, .. }) if *last == local
            );
        if self.funs.tco_peers.is_empty() || !is_block_tail {
            return Ok(false);
        }
        match value {
            Value::Call { fun, args } => {
                if self.funs.tco_peers.contains(fun) {
                    // Only drop roots/frame once the callee is known to exist;
                    // otherwise fall through to a normal call with roots intact.
                    if !self.funs.functions.contains_key(fun) {
                        return Ok(false);
                    }
                    self.root_pop_to(0)?;
                    self.emit_frame_pop()?;
                    let ok = self.emit_musttail_call(fun, args)?;
                    debug_assert!(ok, "musttail callee was declared");
                    return Ok(ok);
                }
            }
            Value::IndirectCall { callee, args } => {
                if let Some(fun) = self.funs.funref_locals.get(&callee.0).cloned() {
                    if self.funs.tco_peers.contains(&fun) {
                        if !self.funs.functions.contains_key(&fun) {
                            return Ok(false);
                        }
                        self.root_pop_to(0)?;
                        self.emit_frame_pop()?;
                        let ok = self.emit_musttail_call(&fun, args)?;
                        debug_assert!(ok, "musttail callee was declared");
                        return Ok(ok);
                    }
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn bind_let_after_emit(
        &mut self,
        local: Local,
        value: &Value,
        v: BasicValueEnum<'ctx>,
    ) -> Result<()> {
        let ty = self.infer_value_ty(value);
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
        if let Value::FunRef(name) = value {
            self.funs.funref_locals.insert(local.0, name.clone());
        } else if let Value::Local(Local(src)) = value {
            if let Some(n) = self.funs.funref_locals.get(src).cloned() {
                self.funs.funref_locals.insert(local.0, n);
            } else {
                self.funs.funref_locals.remove(&local.0);
            }
        } else {
            self.funs.funref_locals.remove(&local.0);
        }
        Ok(())
    }

    /// `Name`/`Local` alias or `AdtField` extract — not a fresh alloc / call result.
    fn value_is_cow_alias(value: &Value) -> bool {
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

    /// Track `Value::Int` / aliases so `AdtField` can resolve `params[idx]`.
    fn note_int_const(&mut self, local: u32, value: &Value) {
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

    /// `xs = xs.set(…)` / `xs = xs.append(…)` — next op assigns this COW result
    /// back onto the loaded slot (unique RC can mutate in place).
    fn cow_reassign_consumes(
        &self,
        block: &Block,
        let_idx: usize,
        dest: Local,
        value: &Value,
    ) -> bool {
        let Value::Builtin { name, args } = value else {
            return false;
        };
        let list_arg = match name {
            lumi_hir::Builtin::MapSet
            | lumi_hir::Builtin::ListAppend
            | lumi_hir::Builtin::ListConcat
            | lumi_hir::Builtin::ListTake
            | lumi_hir::Builtin::ListSlice
            | lumi_hir::Builtin::ListReverse
            | lumi_hir::Builtin::ListSort
            | lumi_hir::Builtin::SetInsert => args.first(),
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
    /// `lumi_adt_ensure_unique_consume_mask` (drops the with-temp `Name(slot)`
    /// retain; overwrite mask skips nested retain on rewritten fields).
    fn match_adt_with_reassign(
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

    /// `slot = <heap expr>` lowered as `Let t = expr; Assign slot := t` with no other uses of `t`.
    /// The mut slot is already a GC root, so the temp need not be shadow-stack rooted.
    fn let_only_feeds_next_assign(&self, block: &Block, let_idx: usize, local: Local) -> bool {
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
    fn let_is_ephemeral_rooted_recv(
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
                    value: Value::Builtin { name, args },
                    ..
                } => {
                    let recv = matches!(
                        name,
                        lumi_hir::Builtin::ListGet
                            | lumi_hir::Builtin::ListLen
                            | lumi_hir::Builtin::ListAppend
                            | lumi_hir::Builtin::ListConcat
                            | lumi_hir::Builtin::ListTake
                            | lumi_hir::Builtin::ListSlice
                            | lumi_hir::Builtin::MapSet
                            | lumi_hir::Builtin::MapRemove
                            | lumi_hir::Builtin::Contains
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
    /// force `ensure_unique` clones inside `lumi_cn_*` / `lumi_f64_*` kernels.
    ///
    /// **Not** applied to `AdtField`: extracting a List/heap field without retain
    /// lets the parent ADT drop while the callee still holds the unreained
    /// pointer (`makeObs` → `nearest(eco, eco.ecoThreats, n)` zeroed threat obs).
    fn let_is_ephemeral_call_arg(
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
                Op::Let { value, .. } | Op::Effect { value } => match value {
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
    fn let_is_ephemeral_adt_field_base(
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
                Op::Let { value, .. } | Op::Effect { value } => match value {
                    Value::Builtin {
                        name: Builtin::AdtField,
                        args,
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
    fn let_is_unused_inplace_with_field(
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
            Op::Let { value, .. } | Op::Effect { value } => Self::value_uses_local(value, local),
            Op::Assign { value, .. } | Op::Return { value } => *value == local,
            Op::Break | Op::Continue => false,
        }
    }

    fn value_uses_local(value: &Value, local: Local) -> bool {
        let mut hit = false;
        lumi_core::for_each_local(value, &mut |l| {
            if l == local {
                hit = true;
            }
        });
        if hit {
            return true;
        }
        let mut nested_hit = false;
        lumi_core::for_each_nested_block(value, &mut |b| {
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

    fn emit_block(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        for (idx, op) in block.ops.iter().enumerate() {
            // If current block already terminated (after break/continue), skip.
            if self
                .llvm
                .builder
                .get_insert_block()
                .and_then(|bb| bb.get_terminator())
                .is_some()
            {
                break;
            }
            match op {
                Op::Let { local, value, .. } => {
                    if self.try_emit_tco_let(block, *local, value)? {
                        return Ok(None);
                    }
                    self.frame.cow_consume_unique =
                        self.cow_reassign_consumes(block, idx, *local, value);
                    self.frame.adt_with_inplace =
                        self.match_adt_with_reassign(block, idx, *local, value);
                    self.frame.emit_dest = Some(local.0);
                    self.frame.expect_alloc_ty =
                        self.peek_expected_alloc_ty(block, idx, *local, value);
                    let v = self.emit_value(value, fv)?;
                    self.frame.expect_alloc_ty = None;
                    self.frame.emit_dest = None;
                    self.frame.cow_consume_unique = false;
                    self.frame.adt_with_inplace = None;
                    if self.let_only_feeds_next_assign(block, idx, *local)
                        || self.let_is_ephemeral_rooted_recv(block, idx, *local, value)
                        || self.let_is_ephemeral_call_arg(block, idx, *local, value)
                        || self.let_is_ephemeral_adt_field_base(block, idx, *local, value)
                        || self.let_is_unused_inplace_with_field(block, idx, *local, value)
                    {
                        // Skip retain+root: source is already live (mut slot / prior let).
                        // Extra retain here inflated COW RC and forced kernel-side clones.
                        let ty = self.infer_value_ty(value);
                        self.frame.locals.insert(local.0, v);
                        self.frame.local_tys.insert(local.0, ty);
                        self.note_int_const(local.0, value);
                        self.funs.funref_locals.remove(&local.0);
                    } else {
                        self.bind_let_after_emit(*local, value, v)?;
                    }
                }
                Op::Effect { value } => {
                    let _ = self.emit_value(value, fv)?;
                }
                Op::Assign { name, value } => {
                    let v = self.local(*value)?;
                    // Float ADT fields / lets travel as i64 IEEE bits. Storing them
                    // into a mut slot via coerce_i64 + Int typing makes later float
                    // arith `sitofp` the bit pattern (eco `var s = eco.ecoRng` bug).
                    // Promote to a native f64 slot whenever the RHS is Float-typed.
                    let v = if matches!(self.frame.local_tys.get(&value.0), Some(Type::Float)) {
                        self.frame.slot_tys.insert(name.clone(), Type::Float);
                        self.promote_f64(v)?.into()
                    } else {
                        if let Some(ty) = self.frame.local_tys.get(&value.0).cloned() {
                            self.frame.slot_tys.insert(name.clone(), ty);
                        }
                        v
                    };
                    self.store_slot(name, v)?;
                }
                Op::Break => {
                    let (_, break_bb, loop_depth) = self
                        .frame
                        .loop_stack
                        .last()
                        .copied()
                        .context("break outside loop")?;
                    self.root_pop_to(loop_depth)?;
                    crate::error::llvm(self.llvm.builder.build_unconditional_branch(break_bb))?;
                }
                Op::Continue => {
                    let (cont_bb, _, loop_depth) = self
                        .frame
                        .loop_stack
                        .last()
                        .copied()
                        .context("continue outside loop")?;
                    self.root_pop_to(loop_depth)?;
                    crate::error::llvm(self.llvm.builder.build_unconditional_branch(cont_bb))?;
                }
                Op::Return { value } => {
                    let v = self.local(*value)?;
                    let ret_i = if matches!(self.frame.local_tys.get(&value.0), Some(Type::Float)) {
                        match v {
                            BasicValueEnum::FloatValue(f) => {
                                crate::error::llvm(self.llvm.builder.build_bit_cast(
                                    f,
                                    self.llvm.i64_ty,
                                    "ret_f64_bits",
                                ))?
                                .into_int_value()
                            }
                            other => self.coerce_i64(other)?,
                        }
                    } else {
                        self.coerce_i64(v)?
                    };
                    #[cfg(feature = "opt-memo")]
                    match self.memo.current_memo {
                        Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_store(id, ret_i)?,
                        Some(MemoTf::Slots { id }) => self.emit_memo_tf_store(id, ret_i)?,
                        None => {}
                    }
                    self.emit_return_i64(ret_i)?;
                }
            }
        }
        if self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some()
        {
            return Ok(None);
        }
        if let Some(r) = block.result {
            Ok(Some(self.local(r)?))
        } else {
            Ok(None)
        }
    }

    /// Emit a nested block and drop roots pushed inside it (unless it terminated
    /// via break/continue, which already restored the loop entry depth).
    pub(crate) fn emit_scoped_block(
        &mut self,
        block: &Block,
        fv: FunctionValue<'ctx>,
    ) -> Result<Option<BasicValueEnum<'ctx>>> {
        let depth = self.frame.root_depth;
        let result = self.emit_block(block, fv)?;
        let terminated = self
            .llvm
            .builder
            .get_insert_block()
            .and_then(|bb| bb.get_terminator())
            .is_some();
        if !terminated {
            self.root_pop_to(depth)?;
        }
        Ok(result)
    }
}

//! Function / block emission and mutable slots.

mod abi;
mod helpers;
mod slots;

use super::Codegen;
use anyhow::{Context as AnyhowContext, Result};
use inkwell::values::{BasicValueEnum, FunctionValue};
use lumia_core::{Block, CoreFun, Local, MemoTf, Op, Value};
use lumia_hir::Builtin;
use lumia_ty::Type;

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
        self.frame.nsw_binop_locals = crate::nsw_iv::collect_nsw_binop_locals(&fun.body);
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

        let compute_bb = match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_prologue(fun, fv, id)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_tf_prologue(fun, fv, id)?,
            None => entry,
        };
        if fun.memo.is_some() {
            self.llvm.builder.position_at_end(compute_bb);
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

        match fun.memo {
            Some(MemoTf::DenseInt { id }) => self.emit_memo_idx_store(id, ret_i)?,
            Some(MemoTf::Slots { id }) => self.emit_memo_tf_store(id, ret_i)?,
            None => {}
        }
        self.emit_return_i64(ret_i)?;
        Ok(())
    }

    pub(crate) fn infer_value_ty(&self, value: &Value) -> Type {
        lumia_core::infer_value_ty_ctx(value, self.infer_ctx(), None)
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
                    self.root_pop_to(0)?;
                    self.emit_frame_pop()?;
                    if self.emit_musttail_call(fun, args)? {
                        return Ok(true);
                    }
                    // musttail failed — restore frame for normal call path.
                    self.emit_frame_push(&self.funs.current_fun.clone())?;
                }
            }
            Value::IndirectCall { callee, args } => {
                if let Some(fun) = self.funs.funref_locals.get(&callee.0).cloned() {
                    if self.funs.tco_peers.contains(&fun) {
                        self.root_pop_to(0)?;
                        self.emit_frame_pop()?;
                        if self.emit_musttail_call(&fun, args)? {
                            return Ok(true);
                        }
                        self.emit_frame_push(&self.funs.current_fun.clone())?;
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
        match value {
            Value::Local(_) | Value::Name(_) => true,
            Value::Builtin {
                name: Builtin::AdtField,
                ..
            } => true,
            _ => false,
        }
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
    /// `lumia_adt_ensure_unique_consume` (drops the with-temp `Name(slot)` retain).
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
        let Op::Assign { name: slot, value: v } = block.ops.get(let_idx + 1)? else {
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

    fn op_uses_local(op: &Op, local: Local) -> bool {
        match op {
            Op::Let { value, .. } | Op::Effect { value } => Self::value_uses_local(value, local),
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
                    let v = self.emit_value(value, fv)?;
                    self.frame.emit_dest = None;
                    self.frame.cow_consume_unique = false;
                    self.frame.adt_with_inplace = None;
                    if self.let_only_feeds_next_assign(block, idx, *local)
                        || self.let_is_ephemeral_rooted_recv(block, idx, *local, value)
                    {
                        // Skip shadow-stack root: next Assign stores into a rooted mut
                        // slot, or this is a Name/Local alias of an already-rooted list
                        // used only as get/set/append receiver.
                        // Still bump COW RC for ADT/List aliases (`var i = o.inner`).
                        let ty = self.infer_value_ty(value);
                        if Self::type_needs_cow_retain(&ty) && Self::value_is_cow_alias(value) {
                            if let Ok(bits) = self.coerce_i64(v) {
                                self.adt_retain_i64(bits)?;
                            }
                        }
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
                    if let Some(ty) = self.frame.local_tys.get(&value.0).cloned() {
                        if !matches!(ty, Type::Float) {
                            self.frame.slot_tys.insert(name.clone(), ty);
                        }
                    }
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

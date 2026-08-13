//! Sub-state for [`crate::Codegen`] — keeps the god-object fields grouped.

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::types::IntType;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use lumia_core::{Local, MemoTf, Value};
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// LLVM context / module / builder / common types.
pub(crate) struct LlvmTypes<'ctx> {
    pub context: &'ctx Context,
    pub module: LlvmModule<'ctx>,
    pub builder: Builder<'ctx>,
    pub i64_ty: IntType<'ctx>,
}

/// Per-function symbol tables and TCO / ABI metadata.
#[derive(Default)]
pub(crate) struct FunTables<'ctx> {
    pub functions: HashMap<String, FunctionValue<'ctx>>,
    pub fun_ret_tys: HashMap<String, Type>,
    pub fun_param_tys: HashMap<String, Vec<Type>>,
    pub fun_param0_identity: HashSet<String>,
    pub external_funs: HashSet<String>,
    /// `foreign` symbols that use the `lumia_rt` object ABI (no String↔cstr).
    pub runtime_external_funs: HashSet<String>,
    pub funref_locals: HashMap<u32, String>,
    pub current_fun: String,
    pub tco_peers: HashSet<String>,
    pub tco_sccs: HashMap<String, HashSet<String>>,
    pub hash_adts: HashSet<String>,
    /// Variant labels by ADT/product type name (tag → display name) for Show.
    pub adt_variant_names: HashMap<String, Vec<String>>,
    /// Stable Show-kind ids (`≥ 1`) packed into ADT `type_id` for recursive `lumia_show`.
    pub adt_show_kinds: HashMap<String, u16>,
}

/// Per-frame SSA / slot / GC-root state while emitting one function.
#[derive(Default)]
pub(crate) struct FrameState<'ctx> {
    pub locals: HashMap<u32, BasicValueEnum<'ctx>>,
    pub slots: HashMap<String, PointerValue<'ctx>>,
    pub float_slots: HashSet<String>,
    pub loop_stack: Vec<(BasicBlock<'ctx>, BasicBlock<'ctx>, u32)>,
    pub local_tys: HashMap<u32, Type>,
    pub slot_tys: HashMap<String, Type>,
    /// Locals bound to `Value::Int(n)` — used to type `AdtField` as `params[n]`.
    pub local_int_consts: HashMap<u32, i64>,
    pub root_depth: u32,
    /// Mut-slot names currently on the shadow stack → `root_depth` right after their push.
    /// Evicted by [`crate::Codegen::root_pop_to`] when that depth is unwound so a later
    /// `ensure_slot` / `load_slot` can re-push (scoped if/loop must not leave stale
    /// "already rooted" compile-time state).
    pub rooted_slots: HashMap<String, u32>,
    pub entry_bb: Option<BasicBlock<'ctx>>,
    /// Dest local of the `Let` currently being emitted (for NSW lookup).
    pub emit_dest: Option<u32>,
    /// `MapSet` may mutate in place: codegen proved `xs = xs.set(…)` consumes the slot.
    pub cow_consume_unique: bool,
    /// `slot = slot with {…}`: mut slot name + updated field indices/locals.
    pub adt_with_inplace: Option<(String, Vec<(u32, Local)>)>,
    /// `Binary` Add/Sub locals proven safe as loop IV `±1` (see `nsw_iv`).
    pub nsw_binop_locals: HashSet<u32>,
    /// Locals safe as `div`/`rem` RHS (const ∉ {0,-1} or always-≥2 slots).
    pub safe_divisor_locals: HashSet<u32>,
    /// `Name(iv)` loads inside loops where the header proves `iv >= 0`.
    pub nonneg_iv_load_locals: HashSet<u32>,
    /// Function-wide `Int`/`Name`/`Binary` Lets (includes LICM'd consts outside loops).
    pub leaf_defs: HashMap<u32, Value>,
    /// Last known const for i64 mut slots (`None` = non-const / unknown).
    /// Used to refuse SR when accumulators/IVs are not at the expected start.
    pub slot_i64_const: HashMap<String, Option<i64>>,
    /// Expected type for the `Let` currently being emitted (from ret / typed slot).
    /// Used so empty `listOf()` / `mapOf()` / `setOf()` keep Float container tags.
    pub expect_alloc_ty: Option<Type>,
}

/// Memo transform emission scratch for the current function.
#[derive(Default)]
pub(crate) struct MemoEmit<'ctx> {
    pub memo_arg_slots: Vec<PointerValue<'ctx>>,
    pub memo_idx_key: Option<PointerValue<'ctx>>,
    pub current_memo: Option<MemoTf>,
}

//! Sub-state for [`crate::Codegen`] — keeps the god-object fields grouped.

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module as LlvmModule;
use inkwell::types::IntType;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use lumia_core::{FunRefAliases, Local, MemoTf, Value};
use lumia_hir::Sym;
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
///
/// ABI types and Core analysis side tables are seeded from
/// [`lumia_core::ModuleTables`] at emit entry; this struct then owns LLVM
/// handles and emit-only side tables (`closure_cap_tys`, `adt_show_kinds`, …).
pub(crate) struct FunTables<'ctx> {
    pub functions: HashMap<Sym, FunctionValue<'ctx>>,
    pub fun_ret_tys: HashMap<Sym, Type>,
    pub fun_param_tys: HashMap<Sym, Vec<Type>>,
    pub fun_param0_identity: HashSet<Sym>,
    pub external_funs: HashSet<String>,
    /// `foreign` symbols that use the `lumia_rt` object ABI (no String↔cstr).
    pub runtime_external_funs: HashSet<String>,
    /// FunRef SSA + named-slot aliases (shared protocol with core directize / TCO).
    pub funref: FunRefAliases,
    pub current_fun: Sym,
    pub tco_peers: HashSet<Sym>,
    pub tco_sccs: HashMap<Sym, HashSet<Sym>>,
    pub hash_adts: HashSet<Sym>,
    /// Variant labels by ADT/product type name (tag → display name) for Show.
    pub adt_variant_names: HashMap<Sym, Vec<String>>,
    /// Sum ADT name → max variant payload arity (shared typed params slots).
    pub sum_max_arity: HashMap<Sym, usize>,
    /// When set, `ChannelNew`/`ChannelRecv` use this elem type (from Core sends).
    pub channel_elem_hint: Option<Type>,
    /// Per-`ChannelNew` local → payload (preferred over module hint when set).
    pub channel_elem_by_local: HashMap<u32, Type>,
    /// `(lifted_fun, capture_index) →` type of the captured local at AllocClosure sites.
    pub closure_cap_tys: HashMap<Sym, HashMap<u32, Type>>,
    /// Stable Show-kind ids (`≥ 1`) packed into ADT `type_id` for recursive `lumia_show`.
    pub adt_show_kinds: HashMap<Sym, u16>,
}

impl<'ctx> Default for FunTables<'ctx> {
    fn default() -> Self {
        Self {
            functions: HashMap::default(),
            fun_ret_tys: HashMap::default(),
            fun_param_tys: HashMap::default(),
            fun_param0_identity: HashSet::default(),
            external_funs: HashSet::default(),
            runtime_external_funs: HashSet::default(),
            funref: FunRefAliases::default(),
            current_fun: Sym::from(""),
            tco_peers: HashSet::default(),
            tco_sccs: HashMap::default(),
            hash_adts: HashSet::default(),
            adt_variant_names: HashMap::default(),
            sum_max_arity: HashMap::default(),
            channel_elem_hint: None,
            channel_elem_by_local: HashMap::default(),
            closure_cap_tys: HashMap::default(),
            adt_show_kinds: HashMap::default(),
        }
    }
}

impl FunTables<'_> {
    /// Seed ABI / analysis blackboard fields from Core [`lumia_core::ModuleTables`].
    /// LLVM handles and emit-only maps (`closure_cap_tys`, `adt_show_kinds`, …) stay here.
    pub(crate) fn seed_abi_from(&mut self, tables: lumia_core::ModuleTables) {
        self.hash_adts = tables.hash_adts;
        self.adt_variant_names = tables.adt_variant_names;
        self.sum_max_arity = tables.sum_max_arity;
        self.channel_elem_hint = tables.channel_elem_hint;
        self.channel_elem_by_local = tables.channel_elem_by_local;
        self.fun_ret_tys = tables.fun_ret_tys;
        self.fun_param_tys = tables.fun_param_tys;
        self.fun_param0_identity = tables.fun_param0_identity;
    }
}

/// Last use of an SSA heap root lies in only one arm of the enclosing `If`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IfArmExclusive {
    Then,
    Else,
}

/// Cross-control-flow last-use site — enables early shadow-stack pop at region exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CrossBlockLastUse {
    IfArm(IfArmExclusive),
    Loop,
    Lambda,
}
#[derive(Clone, Debug)]
pub(crate) struct SsaRoot {
    pub local: lumia_core::Local,
    /// Last op index in the defining block at which this local is still live.
    /// `None` = unused after the def (pop as soon as possible).
    pub last_use: Option<usize>,
    /// When [`Self::last_use`] ends inside a single `If` arm, `Loop`, or `Lambda` region.
    pub last_use_cross: Option<CrossBlockLastUse>,
    /// Index in the mutator ROOTS vec at push time (maintained across swap_remove).
    pub roots_index: usize,
    /// `root_depth` immediately after the matching `root_push` (scope unwind).
    pub depth: u32,
}

/// Per-frame SSA / slot / GC-root state while emitting one function.
#[derive(Default)]
pub(crate) struct FrameState<'ctx> {
    pub locals: HashMap<u32, BasicValueEnum<'ctx>>,
    pub slots: HashMap<Sym, PointerValue<'ctx>>,
    pub float_slots: HashSet<Sym>,
    pub loop_stack: Vec<(BasicBlock<'ctx>, BasicBlock<'ctx>, u32)>,
    pub local_tys: HashMap<u32, Type>,
    pub slot_tys: HashMap<Sym, Type>,
    /// Locals bound to `Value::Int(n)` — used to type `AdtField` as `params[n]`.
    pub local_int_consts: HashMap<u32, i64>,
    pub root_depth: u32,
    /// Mut-slot names currently on the shadow stack → `root_depth` right after their push.
    /// Evicted by [`crate::Codegen::root_pop_to`] when that depth is unwound so a later
    /// `ensure_slot` / `load_slot` can re-push (scoped if/loop must not leave stale
    /// "already rooted" compile-time state).
    ///
    /// Nested `if` snapshots this map at arm entry and restores it after each arm
    /// (musttail may wipe the live map via `root_pop_to(0)`). Cross-block SSA
    /// roots (`If` single arm / `Loop` body) are popped at region exit and the
    /// snapshot is refreshed so restore does not resurrect dead roots.
    pub rooted_slots: HashMap<Sym, u32>,
    /// SSA lets (and heap params) currently on the shadow stack, newest last.
    /// Used to `root_pop` / swap_remove dead roots as soon as last-use passes
    /// (including buried entries under still-live roots). Nested `if` snapshots
    /// this vec with [`rooted_slots`].
    pub ssa_root_stack: Vec<SsaRoot>,
    pub entry_bb: Option<BasicBlock<'ctx>>,
    /// Dest local of the `Let` currently being emitted (for NSW lookup).
    pub emit_dest: Option<u32>,
    /// `MapSet` may mutate in place: codegen proved `xs = xs.set(…)` consumes the slot.
    pub cow_consume_unique: bool,
    /// `slot = slot with {…}`: mut slot name + updated field indices/locals.
    pub adt_with_inplace: Option<(Sym, Vec<(u32, Local)>)>,
    /// `Binary` Add/Sub locals proven safe as loop IV `±1` (opt `nsw_iv` → CoreFun).
    /// Prefer reading via [`Self::install_nsw_from_fun`].
    pub nsw_binop_locals: HashSet<u32>,
    /// Locals safe as `div`/`rem` RHS (const ∉ {0,-1} or always-≥2 slots).
    pub safe_divisor_locals: HashSet<u32>,
    /// `Name(iv)` loads inside loops where the header proves `iv >= 0`.
    pub nonneg_iv_load_locals: HashSet<u32>,
    /// Function-wide `Int`/`Name`/`Binary` Lets (includes LICM'd consts outside loops).
    pub leaf_defs: HashMap<u32, Value>,
    /// Last known const for i64 mut slots (`None` = non-const / unknown).
    /// Used to refuse SR when accumulators/IVs are not at the expected start.
    pub slot_i64_const: HashMap<Sym, Option<i64>>,
    /// Expected type for the `Let` currently being emitted (from ret / typed slot).
    /// Used so empty `listOf()` / `mapOf()` / `setOf()` keep Float/Bool container tags.
    pub expect_alloc_ty: Option<Type>,
}

impl<'ctx> FrameState<'ctx> {
    /// Install NSW sidecar from Core + rebuild emit-local `leaf_defs`.
    pub fn install_nsw_from_fun(&mut self, fun: &lumia_core::CoreFun) {
        self.nsw_binop_locals = fun.nsw_binop_locals.clone();
        self.safe_divisor_locals = fun.safe_divisor_locals.clone();
        self.nonneg_iv_load_locals = fun.nonneg_iv_load_locals.clone();
        self.leaf_defs = lumia_core::collect_leaf_defs(&fun.body, false);
    }
}

/// Memo transform emission scratch for the current function.
#[derive(Default)]
pub(crate) struct MemoEmit<'ctx> {
    pub memo_arg_slots: Vec<PointerValue<'ctx>>,
    pub memo_idx_key: Option<PointerValue<'ctx>>,
    pub current_memo: Option<MemoTf>,
}

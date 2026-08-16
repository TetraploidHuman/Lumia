//! Core IR types, local remapping, and formatting.

use crate::visit::max_local_in_value;
use crate::visit::rewrite_value_locals;
use crate::ops::{CoreBinOp, CoreUnOp};
use lumia_hir::Builtin;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub u32);

/// Calling convention for [`CoreFun::external`] imports.
///
/// Set explicitly at the declare / synth site. Mid/backend must use this field —
/// do not re-derive ABI from the symbol name string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ForeignAbi {
    /// Platform C ABI (`foreign "C"` without a runtime symbol).
    #[default]
    C,
    /// Lumia runtime ABI (i64 / ptr layout matching `lumia_rt`).
    Runtime,
}

#[derive(Debug, Clone)]
pub struct CoreModule {
    pub name: String,
    pub functions: Vec<CoreFun>,
    /// ADT/product type names with `instance Hash` (may use HashOrdered Map/Set).
    pub hash_adts: HashSet<String>,
    /// `(type, method)` → mangled `__Trait_Type_method` (for poly UFCS resolve after mono).
    pub trait_methods: HashMap<(String, String), Vec<String>>,
    /// Variant / product display names by type, indexed by tag (print/`Show` only).
    pub adt_variant_names: HashMap<String, Vec<String>>,
    /// Sum ADT name → max variant payload arity (shared `Type::Adt` params slots).
    pub sum_max_arity: HashMap<String, usize>,
    /// When every channel agrees on a ground payload (typed stamp and/or sends),
    /// module-wide hint for recv/join typing; else per-local map only.
    pub channel_elem_hint: Option<Type>,
    /// Per-`ChannelNew` local id → agreed send payload (locals unique after lift).
    pub channel_elem_by_local: HashMap<u32, Type>,
    /// Same-channel sends that disagreed on payload type `(prev, next)`.
    pub channel_elem_conflicts: Vec<(Type, Type)>,
    /// `Option::Some` ctor tag from the source module (default 0).
    pub option_some_tag: i64,
    /// `Option::None` ctor tag from the source module (default 1).
    pub option_none_tag: i64,
}

impl CoreModule {
    /// Empty module shell (tests / fixtures).
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            functions: Vec::new(),
            hash_adts: HashSet::default(),
            trait_methods: HashMap::default(),
            adt_variant_names: HashMap::default(),
            sum_max_arity: HashMap::default(),
            channel_elem_hint: None,
            channel_elem_by_local: HashMap::default(),
            channel_elem_conflicts: Vec::new(),
            option_some_tag: 0,
            option_none_tag: 1,
        }
    }

    /// Module with only the given functions (common test constructor).
    pub fn with_functions(name: impl Into<String>, functions: Vec<CoreFun>) -> Self {
        Self {
            functions,
            ..Self::empty(name)
        }
    }

    /// Reject same-channel mixed payloads (hints cannot recover ABI).
    pub fn check_channel_elem_conflicts(&self) -> Result<(), String> {
        let Some((a, b)) = self.channel_elem_conflicts.first() else {
            return Ok(());
        };
        Err(format!(
            "mixed payloads on one channel ({a} then {b}); use separate channels or a uniform element type"
        ))
    }
}

#[derive(Debug, Clone)]
pub struct CoreFun {
    pub name: String,
    pub params: Vec<Local>,
    pub param_names: Vec<String>,
    /// Parameter types (for float ABI / future typed SSA).
    pub param_tys: Vec<Type>,
    pub body: Block,
    pub ret_ty: Type,
    pub effect: Effect,
    pub is_main: bool,
    /// Transparent Memo `T_f` (DESIGN §7.5.1-B). `None` = capacity 0.
    pub memo: Option<MemoTf>,
    /// When set, this is a C ABI import (`foreign`); no body emitted.
    pub external: Option<String>,
    /// ABI for [`Self::external`]. Ignored when `external` is `None`.
    pub foreign_abi: ForeignAbi,
    /// Locals that may escape (always filled by EscapePass before ReprSelect).
    pub escaping: HashSet<Local>,
    /// HM scheme needs call-site clones (`∀` / Num / trait preds), or signature still open.
    pub scheme_poly: bool,
    /// When set, this function is a monomorphization clone of the named original.
    /// Prefer this over parsing `$` out of [`Self::name`].
    pub mono_of: Option<String>,
    /// Structured identity — prefer over `__lam_` / `__val_` name prefixes.
    pub kind: FunKind,
}

/// How a [`CoreFun`] was introduced (avoids string-prefix protocols).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FunKind {
    #[default]
    Normal,
    /// Lambda lifted by `lambda_lift` (`__lam_*` name is historical).
    LiftedLambda,
    /// Module-level `val` getter (`__val_*` name is historical).
    ValGetter,
}

impl CoreFun {
    #[inline]
    pub fn is_mono_clone(&self) -> bool {
        self.mono_of.is_some()
    }

    /// Lifted nested lambda (env-bearing or FunRef).
    ///
    /// Relies on [`FunKind::LiftedLambda`] set at lift time — do not parse
    /// `__lam_` prefixes in mid/backend.
    #[inline]
    pub fn is_lifted_lambda(&self) -> bool {
        matches!(self.kind, FunKind::LiftedLambda)
    }

    /// Module-level `val` getter ([`FunKind::ValGetter`] set at lower).
    #[inline]
    pub fn is_val_getter(&self) -> bool {
        matches!(self.kind, FunKind::ValGetter)
    }

    /// Original name before mono clone when [`Self::mono_of`] is set.
    #[inline]
    pub fn base_name(&self) -> &str {
        self.mono_of.as_deref().unwrap_or(self.name.as_str())
    }
}

/// Bounded cross-call memo table — one mechanism, representation is a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoTf {
    /// Fixed small associative table (hot keys / irregular args).
    Slots { id: u32 },
    /// Dense Int domain `[0, CAP)` — prefer for structural recursion on `n`.
    DenseInt { id: u32 },
}

#[derive(Debug, Clone)]
pub struct Block {
    pub ops: Vec<Op>,
    /// Result local (or unit)
    pub result: Option<Local>,
}

#[derive(Debug, Clone)]
pub enum Op {
    Let {
        local: Local,
        value: Value,
        /// Pure region marker for §7.4.1
        pure_region: bool,
    },
    Assign {
        name: String,
        value: Local,
    },
    Break,
    Continue,
    /// Early return from the current Core function.
    Return {
        value: Local,
    },
}

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Unit,
    Local(Local),
    /// Named mutable/immutable binding load
    Name(String),
    Binary {
        op: CoreBinOp,
        left: Local,
        right: Local,
    },
    Unary {
        op: CoreUnOp,
        operand: Local,
    },
    Call {
        fun: String,
        args: Vec<Local>,
    },
    /// Call through a function pointer (first-class / lifted lambda).
    IndirectCall {
        callee: Local,
        args: Vec<Local>,
    },
    Builtin {
        name: Builtin,
        args: Vec<Local>,
        /// Optional result type stamped from HIR `type_at` (e.g. ground `Channel[T]`).
        result_ty: Option<Type>,
    },
    If {
        cond: Local,
        then_block: Box<Block>,
        else_block: Box<Block>,
    },
    /// While-style: `header` recomputes condition; `body`; `latch` runs before next header
    /// (`continue` → latch; normal body end → latch).
    Loop {
        header: Box<Block>,
        body: Box<Block>,
        latch: Box<Block>,
    },
    Lambda {
        params: Vec<Local>,
        body: Box<Block>,
    },
    /// Pointer to a known Core/LLVM function (as i64).
    FunRef(String),
    /// Representation hint after lowering (default path)
    AllocList {
        elems: Vec<Local>,
        repr: ListRepr,
    },
    AllocSet {
        elems: Vec<Local>,
        repr: SetRepr,
    },
    /// Empty or flat key/value locals: [k0,v0,k1,v1,...]
    AllocMap {
        flat_pairs: Vec<Local>,
        repr: MapRepr,
    },
    /// Sum/product: `[tag:i64][field0]…` (`adt_name` for Show overrides / typing).
    AllocAdt {
        adt_name: String,
        tag: i64,
        fields: Vec<Local>,
        repr: AdtRepr,
    },
    /// Heap closure: `[fn_ptr:i64][cap0]…` — `fun` takes `(env, …params)`.
    AllocClosure {
        fun: String,
        captures: Vec<Local>,
    },
    /// Load capture word `index` from closure env (`env` is the heap ptr as i64).
    /// `as_float` restores IEEE bits to f64 in codegen (captures are stored as i64).
    ClosureCap {
        env: Local,
        index: u32,
        as_float: bool,
    },
}

/// List representation hint on `AllocList` (§3.5 / §7.1.1).
/// Runtime Iota ranges use `TYPE_LIST_IOTA` via `lumia_range`, not this enum.
/// Deforestation lives in HIR (`try_fuse_hof_*`), not as an AllocList tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRepr {
    /// Default heap `[len][elems…]`.
    HeapList,
    /// Empty → immortal `lumia_list_empty`; small non-escaping → stack header+payload.
    LitList,
}

/// Map default path.
///
/// [`MapRepr::LitMap`] is a **partial-eval hint** (known constant pairs), not an
/// emit layout — [`crate`]'s ReprSelect lowers it to [`SmallMap`] / hash before
/// codegen. Codegen never stacks maps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapRepr {
    HashOrdered,
    SmallMap,
    /// Eq-only / no Hash — stay linear forever (DESIGN §3.5.1 AssocList).
    AssocList,
    /// PE / memo fold tag only — not a physical layout.
    LitMap,
}

impl MapRepr {
    /// True when this tag is only meaningful to PE / const-fold, not emit.
    #[inline]
    pub fn is_pe_hint(self) -> bool {
        matches!(self, Self::LitMap)
    }
}

/// Set representation hint on `AllocSet`.
///
/// [`SetRepr::LitSet`] is a PE hint; ReprSelect always emits [`HeapSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRepr {
    HeapSet,
    /// PE / memo fold tag only — not a physical layout.
    LitSet,
}

impl SetRepr {
    #[inline]
    pub fn is_pe_hint(self) -> bool {
        matches!(self, Self::LitSet)
    }
}

/// ADT/product representation hint on `AllocAdt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdtRepr {
    /// Default heap `[tag][fields…]`.
    HeapAdt,
    /// Small non-escaping literal → stack header+payload (like `ListRepr::LitList`).
    LitAdt,
}

pub(crate) fn max_local_in_module(module: &CoreModule) -> u32 {
    let mut max = 0u32;
    for fun in &module.functions {
        max = max.max(max_local_in_fun(fun));
    }
    max
}

/// Highest `Local` id used in a function (params + body).
pub fn max_local_in_fun(fun: &CoreFun) -> u32 {
    let mut max = 0u32;
    for p in &fun.params {
        max = max.max(p.0);
    }
    max.max(max_local_in_block(&fun.body))
}

pub fn max_local_in_block(block: &Block) -> u32 {
    let mut max = 0u32;
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                max = max.max(local.0);
                max = max.max(max_local_in_value(value));
            }
            Op::Assign { value, .. } | Op::Return { value } => max = max.max(value.0),
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = &block.result {
        max = max.max(r.0);
    }
    max
}

pub fn rewrite_block_locals(block: &mut Block, remap: &HashMap<u32, u32>) {
    if remap.is_empty() {
        return;
    }
    let map_l = |l: &mut Local| {
        if let Some(&r) = remap.get(&l.0) {
            *l = Local(r);
        }
    };
    if let Some(r) = &mut block.result {
        map_l(r);
    }
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                map_l(local);
                rewrite_value_locals(value, remap);
            }
            Op::Assign { value, .. } | Op::Return { value } => map_l(value),
            Op::Break | Op::Continue => {}
        }
    }
}

/// Display Core IR for `--show-ir`.
pub fn format_module(m: &CoreModule) -> String {
    let mut out = String::new();
    out.push_str(&format!("module {}\n", m.name));
    for f in &m.functions {
        out.push_str(&format!(
            "\nfun {}({}) effect.io={} memo={:?} {{\n",
            f.name,
            f.param_names.join(", "),
            f.effect.has_io(),
            f.memo
        ));
        format_block(&f.body, &mut out, 1);
        out.push_str("}\n");
    }
    out
}

fn format_block(b: &Block, out: &mut String, indent: usize) {
    let pad = "  ".repeat(indent);
    for op in &b.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } => {
                out.push_str(&format!(
                    "{pad}%{} = {}{}\n",
                    local.0,
                    format_value(value),
                    if *pure_region {
                        "  // pure"
                    } else {
                        "  // effect"
                    }
                ));
            }
            Op::Assign { name, value } => {
                out.push_str(&format!("{pad}{name} := %{}\n", value.0));
            }
            Op::Break => out.push_str(&format!("{pad}break\n")),
            Op::Continue => out.push_str(&format!("{pad}continue\n")),
            Op::Return { value } => out.push_str(&format!("{pad}early_return %{}\n", value.0)),
        }
    }
    if let Some(r) = b.result {
        out.push_str(&format!("{pad}return %{}\n", r.0));
    }
}

fn format_value(v: &Value) -> String {
    match v {
        Value::Int(n) => format!("i64 {n}"),
        Value::Float(n) => format!("f64 {n}"),
        Value::Bool(b) => format!("bool {b}"),
        Value::String(s) => format!("str \"{s}\""),
        Value::Char(c) => format!("char {c:?}"),
        Value::Unit => "unit".into(),
        Value::Local(l) => format!("%{}", l.0),
        Value::Name(n) => format!("load {n}"),
        Value::Binary { op, left, right } => {
            format!("%{} {op} %{}", left.0, right.0)
        }
        Value::Unary { op, operand } => format!("{op:?} %{}", operand.0),
        Value::Call { fun, args } => {
            let a: Vec<_> = args.iter().map(|l| format!("%{}", l.0)).collect();
            format!("call {fun}({})", a.join(", "))
        }
        Value::IndirectCall { callee, args } => {
            let a: Vec<_> = args.iter().map(|l| format!("%{}", l.0)).collect();
            format!("icall %{}({})", callee.0, a.join(", "))
        }
        Value::Builtin { name, args, .. } => {
            let a: Vec<_> = args.iter().map(|l| format!("%{}", l.0)).collect();
            format!("builtin {name:?}({})", a.join(", "))
        }
        Value::If { .. } => "if ...".into(),
        Value::Loop { .. } => "loop ...".into(),
        Value::Lambda { .. } => "lambda ...".into(),
        Value::FunRef(n) => format!("funref {n}"),
        Value::AllocClosure { fun, captures } => {
            format!("alloc_closure({fun}, n={})", captures.len())
        }
        Value::ClosureCap {
            env,
            index,
            as_float,
        } => {
            if *as_float {
                format!("closure_cap_f(%{}, {index})", env.0)
            } else {
                format!("closure_cap(%{}, {index})", env.0)
            }
        }
        Value::AllocList { elems, repr } => {
            format!("alloc_list[{repr:?}](n={})", elems.len())
        }
        Value::AllocSet { elems, repr } => {
            format!("alloc_set[{repr:?}](n={})", elems.len())
        }
        Value::AllocMap { flat_pairs, repr } => {
            format!("alloc_map[{repr:?}](n={})", flat_pairs.len() / 2)
        }
        Value::AllocAdt {
            adt_name,
            tag,
            fields,
            ..
        } => {
            format!("alloc_adt({adt_name}, tag={tag}, n={})", fields.len())
        }
    }
}

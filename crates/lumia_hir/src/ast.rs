//! HIR AST — modules, expressions, builtins, ADTs.

use lumia_syntax::{BinOp, Span, UnOp};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub items: Vec<Item>,
    /// Sum types declared in this module.
    pub adts: Vec<AdtDef>,
    /// Product types declared in this module.
    pub products: Vec<ProductDef>,
    /// `(trait, type)` pairs from `instance Trait for Type { }` (incl. auto-derived).
    pub instances: HashSet<(String, String)>,
    /// Instance / default methods: `(type, method)` → mangled `__Trait_Type_method`.
    /// Used for UFCS `x.method(...)` resolution (compile-time; DESIGN §6.2).
    /// Show overrides are looked up as `("T", "show")` (no separate Show side table).
    pub trait_methods: HashMap<(String, String), Vec<String>>,
    /// Short method name → declaring trait (from `trait` items; poly UFCS constraints).
    pub method_traits: HashMap<String, String>,
}

impl Module {
    /// Field names for a product type declared in this module.
    pub fn product_fields(&self, name: &str) -> Option<Vec<String>> {
        self.products
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.fields.clone())
    }
}

#[derive(Debug, Clone)]
pub struct ProductDef {
    pub name: String,
    pub fields: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AdtDef {
    pub name: String,
    pub variants: Vec<AdtVariant>,
}

#[derive(Debug, Clone)]
pub struct AdtVariant {
    pub name: String,
    pub tag: i64,
    pub arity: usize,
}

#[derive(Debug, Clone)]
pub struct CtorInfo {
    pub adt_name: String,
    pub tag: i64,
    pub arity: usize,
}

#[derive(Debug, Clone)]
pub enum Item {
    Fun(Fun),
    /// Non-function `val` at module level
    Val {
        name: String,
        body: Expr,
        /// Optional `val x: Int = …` ascription.
        ty: Option<String>,
        /// Declaration span (`val` …), not the body.
        span: Span,
        /// `priv val` — not re-exported via import (mirrors syntax).
        is_priv: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Fun {
    pub name: String,
    pub params: Vec<String>,
    /// Parallel to `params`; empty = all inferred.
    pub param_ann: Vec<Option<String>>,
    /// Optional return ascription from `val f: Ret = { … }` / `val f: Ret (x) = …`.
    pub ret_ann: Option<String>,
    pub body: Expr,
    /// Declaration span (`val` / `foreign`), not the body.
    pub span: Span,
    /// True if this is the program entry `main`
    pub is_main: bool,
    /// C ABI symbol when declared via `foreign "C" fn …`
    pub external: Option<String>,
    /// When `external` is set: (param type names, return type name), e.g. `Int`.
    pub foreign_sig: Option<(Vec<String>, String)>,
    /// `foreign "C" pure fn` → Effect::pure() only when trust is enabled.
    pub foreign_pure: bool,
    /// `priv val` / private function — not re-exported via import.
    pub is_priv: bool,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(String, Span),
    Char(char, Span),
    Unit(Span),
    Var(String, Span),
    Let {
        name: String,
        value: Box<Expr>,
        body: Box<Expr>,
        mutable: bool,
        /// Optional ascription from `val x: T = …` / `var x: T = …`.
        ty: Option<String>,
    },
    Assign {
        name: String,
        value: Box<Expr>,
        span: Span,
    },
    Lambda {
        params: Vec<String>,
        /// Parallel to `params`; empty = all inferred.
        param_ann: Vec<Option<String>>,
        body: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
        span: Span,
    },
    /// `for cond { body }` — while-style loop (Unit result).
    /// `step` runs after each body (and on `continue`) before re-checking `cond`.
    Loop {
        cond: Box<Expr>,
        body: Box<Expr>,
        step: Option<Box<Expr>>,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    /// Limited early return from the nearest function/closure.
    Return {
        value: Box<Expr>,
        span: Span,
    },
    /// Option/Result recovery; desugared after typecheck.
    Alt {
        scrutinee: Box<Expr>,
        alt: Box<Expr>,
        span: Span,
    },
    /// Product update whose type is not unique from field names alone.
    /// Desugared after typecheck once the base expression's ADT is known.
    With {
        base: Box<Expr>,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    Seq {
        stmts: Vec<Expr>,
        span: Span,
    },
    /// Builtin recognized early
    BuiltinCall {
        name: Builtin,
        args: Vec<Expr>,
        span: Span,
    },
    /// Sum-type constructor: heap `[tag][payload…]`.
    AdtNew {
        adt_name: String,
        variant: String,
        tag: i64,
        args: Vec<Expr>,
        span: Span,
    },
}

/// Dispatch family for typing / codegen — keep ty and codegen routers in sync via [`Builtin::family`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFamily {
    Io,
    List,
    MapSet,
    String,
    Adt,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    Println,
    ListLen,
    ListGet,
    ListSlice,
    ListAppend,
    /// `xs.concat(ys)` → new List.
    ListConcat,
    /// `m.contains(k)` / `s.contains(x)` — runtime dispatches on type_id.
    Contains,
    /// Immutable map upsert: `m.set(k, v)` → new Map.
    MapSet,
    /// Immutable delete: `m.remove(k)` / `s.remove(x)` → new Map/Set.
    MapRemove,
    /// Immutable set add: `s.insert(x)` → new Set (no-op if already present).
    SetInsert,
    MapKeys,
    MapValues,
    MapItems,
    /// List/Set identity (heap list); Map → keys. Used by indexed `for` / `toList`.
    Elems,
    Range,
    RangeInclusive,
    /// Format any scalar / String / Char as a heap String (interpolation).
    Show,
    /// String ops.
    StrTrim,
    StrSplit,
    StrSubstring,
    StrToLower,
    StrToUpper,
    StrStartsWith,
    StrEndsWith,
    /// Read entire stdin → String (IO).
    ReadStdin,
    /// Non-exhaustive / failed match (runtime abort).
    MatchFail,
    /// `xs.take(n)` → prefix List.
    ListTake,
    /// `xs.reverse()` → new List (same element order reversed).
    ListReverse,
    /// `xs.sort()` → new List[Int] ascending.
    ListSort,
    /// `xs.sortBy(f)` → permute by Ord keys (stable); runtime takes (values, keys).
    ListSortByKeys,
    /// Auto-parallel candidate `xs.map(f)` (FunRef-safe); demoted if impure/non-scalar.
    ListParMap,
    /// Auto-parallel candidate `xs.fold(z, f)` (FunRef-safe 2-arg; `f` assumed associative).
    ListParFold,
    /// `assert(cond)` — abort if false (programming error).
    Assert,
    /// `xs.join(sep)` for List[String] → String.
    ListJoin,
    /// ADT tag / payload access (match desugar).
    AdtTag,
    AdtField,
    /// `channel(cap)` → Channel[α].
    ChannelNew,
    /// `ch.send(v)`.
    ChannelSend,
    /// `ch.recv()`.
    ChannelRecv,
    /// `ch.recvOpt()` → Option[T].
    ChannelRecvOpt,
    /// `ch.close()`.
    ChannelClose,
    /// `t.join()`.
    TaskJoin,
    /// `t.joinOpt()` → Option[T] (None if cancelled).
    TaskJoinOpt,
    /// Spawn fiber (fnptr + env) — syntax desugar later.
    TaskSpawn,
    /// Enter structured-concurrency scope (scheduler kind).
    ScopeEnter,
    /// Leave current scope (join children).
    ScopeLeave,
    /// Cancel children of the current scope (recoverable; leave soft-awaits).
    ScopeCancel,
}

// [`Builtin::family`] / [`Builtin::info`] live in `builtin_info.rs`.

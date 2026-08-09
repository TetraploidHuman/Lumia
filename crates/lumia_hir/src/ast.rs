//! HIR AST — modules, expressions, builtins, ADTs.

use lumia_syntax::{BinOp, Span, UnOp};
use std::collections::{HashMap, HashSet};

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
    /// Custom `Show.show` overrides: type name → mangled function name.
    pub show_methods: HashMap<String, String>,
    /// Instance / default methods: `(type, method)` → mangled `__Trait_Type_method`.
    /// Used for UFCS `x.method(...)` resolution (compile-time; DESIGN §6.2).
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
    },
}

#[derive(Debug, Clone)]
pub struct Fun {
    pub name: String,
    pub params: Vec<String>,
    pub body: Expr,
    /// True if this is the program entry `main`
    pub is_main: bool,
    /// C ABI symbol when declared via `foreign "C" fn …`
    pub external: Option<String>,
    /// When `external` is set: (param type names, return type name), e.g. `Int`.
    pub foreign_sig: Option<(Vec<String>, String)>,
    /// `foreign "C" pure fn` → Effect::pure() only when trust is enabled.
    pub foreign_pure: bool,
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
    },
    Assign {
        name: String,
        value: Box<Expr>,
        span: Span,
    },
    Lambda {
        params: Vec<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Println,
    PrintlnInt,
    PrintlnStr,
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
}

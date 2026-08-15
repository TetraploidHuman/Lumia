//! Core type, effect, and scheme definitions.

use lumia_hir::{Expr, Module};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use thiserror::Error;

/// Cross-file name visibility after import inlining (entry must not see `priv`).
#[derive(Debug, Clone, Default)]
pub struct NameVisibility {
    pub name_origin: HashMap<String, u32>,
    pub cross_file_visible: HashSet<String>,
    pub entry_file: u32,
}

impl NameVisibility {
    /// Entry module may only name locally declared or explicitly imported symbols.
    /// Dependency modules (inlined for linking) may use the full inlined namespace
    /// so public APIs can call their private/sibling helpers.
    pub fn allows(&self, name: &str, from_file: u32) -> bool {
        if self.name_origin.is_empty() {
            return true;
        }
        if from_file != self.entry_file {
            return true;
        }
        match self.name_origin.get(name) {
            Some(&origin) if origin == from_file => true,
            Some(_) => self.cross_file_visible.contains(name),
            None => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Char,
    Unit,
    Fun(Vec<Type>, Box<Type>, Effect),
    Var(u32),
    /// List[T]
    List(Box<Type>),
    /// Map[K, V]
    Map(Box<Type>, Box<Type>),
    /// Set[T]
    Set(Box<Type>),
    /// Task[T] — fiber handle (Io concurrency).
    Task(Box<Type>),
    /// Channel[T] — bounded message channel.
    Channel(Box<Type>),
    /// Nominal sum type, e.g. Option[T] → Adt("Option", [T]).
    Adt {
        name: String,
        params: Vec<Type>,
    },
    /// `(T1, T2, …)` — fixed arity.
    Tuple(Vec<Type>),
    /// Open tuple prefix: unifies with any [`Type::Tuple`] (or longer prefix) of
    /// length ≥ `prefix.len()` whose leading elements unify. Used for positional
    /// `.0`/`.n` on open receivers without freezing arity (DESIGN / sortBy).
    TuplePrefix(Vec<Type>),
}

/// Effect set ε — empty = pure; `Var` is open during inference (zonked to Pure if unconstrained).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effect {
    #[default]
    Pure,
    Io,
    Var(u32),
}

impl Effect {
    pub fn pure() -> Self {
        Self::Pure
    }
    pub fn io() -> Self {
        Self::Io
    }
    pub fn is_pure(self) -> bool {
        matches!(self, Self::Pure)
    }
    /// Concrete IO bit (unbound `Var` counts as not-yet-IO).
    pub fn has_io(self) -> bool {
        matches!(self, Self::Io)
    }
    /// Shallow lub without linking open vars. Inference uses `Infer::union_eff`
    /// so distinct `Var`s stay constrained together.
    pub fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Io, _) | (_, Self::Io) => Self::Io,
            (Self::Var(v), Self::Pure) | (Self::Pure, Self::Var(v)) => Self::Var(v),
            // Distinct open vars cannot be linked here; keep the first and rely on
            // `Infer::union_eff` at inference sites (see that method).
            (Self::Var(a), Self::Var(_)) => Self::Var(a),
            (Self::Pure, Self::Pure) => Self::Pure,
        }
    }
}

#[derive(Debug, Error)]
pub enum TypeError {
    #[error("{0}")]
    Message(String),
    #[error("{message}")]
    Located {
        span: lumia_syntax::Span,
        message: String,
    },
}

impl From<lumia_syntax::LocatedError> for TypeError {
    fn from(e: lumia_syntax::LocatedError) -> Self {
        TypeError::Located {
            span: e.span,
            message: e.message,
        }
    }
}

impl TypeError {
    pub fn span(&self) -> Option<lumia_syntax::Span> {
        match self {
            TypeError::Located { span, .. } => Some(*span),
            TypeError::Message(_) => None,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            TypeError::Message(m) | TypeError::Located { message: m, .. } => m,
        }
    }
}

pub(crate) fn at(span: lumia_syntax::Span, msg: impl Into<String>) -> TypeError {
    TypeError::Located {
        span,
        message: msg.into(),
    }
}

/// Source span for a HIR expression (walks into `Let`, which has no own span).
pub fn expr_span(e: &Expr) -> lumia_syntax::Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Bool(_, s)
        | Expr::String(_, s)
        | Expr::Char(_, s)
        | Expr::Unit(s)
        | Expr::Var(_, s)
        | Expr::Break(s)
        | Expr::Continue(s) => *s,
        Expr::Assign { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::Call { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Unary { span, .. }
        | Expr::If { span, .. }
        | Expr::Loop { span, .. }
        | Expr::Seq { span, .. }
        | Expr::BuiltinCall { span, .. }
        | Expr::AdtNew { span, .. }
        | Expr::Return { span, .. }
        | Expr::Alt { span, .. }
        | Expr::With { span, .. } => *span,
        Expr::Let { value, .. } => expr_span(value),
    }
}

pub(crate) fn locate(span: lumia_syntax::Span, err: TypeError) -> TypeError {
    match err {
        TypeError::Located { .. } => err,
        TypeError::Message(message) => TypeError::Located { span, message },
    }
}

#[derive(Debug, Clone)]
pub struct TypedModule {
    pub module: Module,
    pub fun_types: HashMap<String, Type>,
    /// Top-level HM schemes after generalize (drives Core monomorphization).
    pub fun_schemes: HashMap<String, Scheme>,
    pub main_effect: Effect,
    /// Expr span → pruned type (for LSP hover).
    pub type_at: Vec<(lumia_syntax::Span, Type)>,
    /// Top-level / local binding name → declaration span (for go-to-def).
    pub decls: HashMap<String, lumia_syntax::Span>,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Debug / error messages: keep `?N` for unbound vars (empty name map).
        // IDE paths use [`crate::display_type`] for grounded Num + letter names.
        let names = rustc_hash::FxHashMap::default();
        write!(f, "{}", super::display::pretty_type_with(self, &names))
    }
}

/// Hindley–Milner type scheme `∀ vars eff_vars. ty` (DESIGN §3.1 let-polymorphism).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scheme {
    pub vars: Vec<u32>,
    pub eff_vars: Vec<u32>,
    pub ty: Type,
    /// Quantified vars that appeared in arithmetic (Num MVP: Int|Float only).
    pub num_vars: Vec<u32>,
    /// Quantified vars used in ordering ops (Ord: scalars or `instance Ord`).
    pub ord_vars: Vec<u32>,
    /// Quantified vars used in `==`/`!=` (must not become Fun).
    pub eq_vars: Vec<u32>,
    /// Quantified vars used with `.len()` (List/Set/Map/String).
    pub len_vars: Vec<u32>,
    /// Quantified vars used with open `.concat` (List or String).
    pub concat_vars: Vec<u32>,
    /// Quantified vars used with `.contains` (Map/Set/String).
    pub contains_vars: Vec<u32>,
    /// Quantified vars used with open `.set` (List or Map).
    pub set_vars: Vec<u32>,
    /// Quantified vars used with `Elems` / `.toList` (List/Set/Map).
    pub elems_vars: Vec<u32>,
    /// Quantified vars used with open `.take` / `.drop` / `.reverse` (List or String).
    pub take_vars: Vec<u32>,
    /// Quantified vars that require `instance Trait` (deferred UFCS on poly params).
    /// Entries: (var, trait_name, method_name).
    pub trait_preds: Vec<(u32, String, String)>,
}

impl Scheme {
    pub fn mono(ty: Type) -> Self {
        Self {
            vars: Vec::new(),
            eff_vars: Vec::new(),
            ty,
            num_vars: Vec::new(),
            ord_vars: Vec::new(),
            eq_vars: Vec::new(),
            len_vars: Vec::new(),
            concat_vars: Vec::new(),
            contains_vars: Vec::new(),
            set_vars: Vec::new(),
            elems_vars: Vec::new(),
            take_vars: Vec::new(),
            trait_preds: Vec::new(),
        }
    }

    /// Whether Core should clone this binder at ground call sites.
    pub fn needs_mono(&self) -> bool {
        !self.vars.is_empty()
            || !self.num_vars.is_empty()
            || !self.ord_vars.is_empty()
            || !self.eq_vars.is_empty()
            || !self.len_vars.is_empty()
            || !self.concat_vars.is_empty()
            || !self.contains_vars.is_empty()
            || !self.set_vars.is_empty()
            || !self.elems_vars.is_empty()
            || !self.take_vars.is_empty()
            || !self.trait_preds.is_empty()
    }
}

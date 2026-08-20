//! Syntax-level AST types.

use crate::span::Span;
use crate::sym::Sym;
use std::fmt;

/// Parsed module AST (syntax level).
#[derive(Debug, Clone)]
pub struct Module {
    pub name: Sym,
    pub span: Span,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub path: Vec<Sym>,
    pub names: ImportNames,
    pub span: Span,
}

/// One imported name, optionally renamed (`name as alias`).
#[derive(Debug, Clone)]
pub struct ImportedName {
    /// Name as exported by the source module.
    pub name: Sym,
    /// Local name in the importer; `None` means same as `name`.
    pub alias: Option<Sym>,
}

impl ImportedName {
    pub fn new(name: impl Into<Sym>) -> Self {
        Self {
            name: name.into(),
            alias: None,
        }
    }

    pub fn with_alias(name: impl Into<Sym>, alias: impl Into<Sym>) -> Self {
        Self {
            name: name.into(),
            alias: Some(alias.into()),
        }
    }

    /// Name used in the importing module.
    pub fn local(&self) -> &str {
        self.alias.as_deref().unwrap_or(self.name.as_str())
    }
}

#[derive(Debug, Clone)]
pub enum ImportNames {
    /// `import a.b` / `import a.b as bee`
    Single(ImportedName),
    /// `import a.{b, c as d}`
    Selective(Vec<ImportedName>),
    /// `import a.*`
    All,
}

#[derive(Debug, Clone)]
pub enum Item {
    Val(ValItem),
    Type(TypeItem),
    /// `foreign "C" fn name(x: Int) -> Int`
    Foreign(ForeignItem),
    /// `trait Eq { }` / `trait Ord requires Eq { val less = … }`.
    Trait(TraitItem),
    /// `instance Eq for Point { }` / with optional `val` method bodies.
    Instance(InstanceItem),
}

#[derive(Debug, Clone)]
pub struct TraitItem {
    pub name: Sym,
    pub requires: Vec<Sym>,
    /// Optional default method bodies (`val name = …`).
    pub methods: Vec<ValItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct InstanceItem {
    pub trait_name: Sym,
    pub type_name: Sym,
    /// Method overrides (`val show = { self -> … }`).
    pub methods: Vec<ValItem>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ForeignItem {
    pub abi: String,
    pub name: Sym,
    pub params: Vec<(Sym, String)>,
    pub ret: String,
    /// `foreign "C" pure fn` — Pure only with `--trust-foreign-pure`.
    pub is_pure: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ValItem {
    pub name: Sym,
    /// Optional ascription on the binding (`val x: Int = …`).
    pub ty: Option<String>,
    /// Optional paren params; each may carry `name: Type`.
    pub params: Option<Vec<(Sym, Option<String>)>>,
    pub body: Expr,
    pub span: Span,
    /// `priv val` — not re-exported via import.
    pub is_priv: bool,
}

#[derive(Debug, Clone)]
pub struct TypeItem {
    pub name: Sym,
    pub kind: TypeKind,
    pub span: Span,
    pub is_priv: bool,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    /// Product: `val` fields
    Product(Vec<Sym>),
    /// Sum: variants
    Sum(Vec<Variant>),
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: Sym,
    pub fields: VariantFields,
}

#[derive(Debug, Clone)]
pub enum VariantFields {
    Unit,
    /// Positional payloads keep binder names from source (`Some(value)` → `["value"]`).
    Positional(Vec<Sym>),
    Named(Vec<Sym>),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(Sym, Span),
    /// Desugared interpolation: `"a${x}b"` → parts lit/expr alternating.
    Interp {
        parts: Vec<InterpPart>,
        span: Span,
    },
    Char(char, Span),
    Ident(Sym, Span),
    /// Block: statements + optional trailing expr value
    Block {
        stmts: Vec<Stmt>,
        tail: Option<Box<Expr>>,
        span: Span,
    },
    /// `{ a, b -> body }` or `{ a: Int, b: Int -> body }` or bare `{ it + 1 }`
    /// (desugared to params=`["it"]` with `bare_it`).
    Lambda {
        params: Vec<Sym>,
        /// Parallel to `params`; `None` = infer. Empty vec means all inferred.
        param_tys: Vec<Option<String>>,
        /// Written as `{ …it… }` without `it ->` (parser invented the param).
        bare_it: bool,
        body: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// Trailing closure desugared into Call before HIR, or kept here
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
        else_branch: Option<Box<Expr>>,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// Kotlin-style subjectless `match { cond -> …; _ -> … }`.
    MatchCond {
        arms: Vec<MatchCondArm>,
        span: Span,
    },
    /// Limited early return from the nearest function/closure (`return expr`).
    Return {
        value: Box<Expr>,
        span: Span,
    },
    /// `scrutinee alt rhs` — Option/Result recovery (rhs is expr or block).
    Alt {
        scrutinee: Box<Expr>,
        alt: Box<Expr>,
        span: Span,
    },
    Field {
        base: Box<Expr>,
        field: Sym,
        span: Span,
    },
    ListLit {
        elems: Vec<Expr>,
        span: Span,
    },
    /// `(a, b, …)` — at least two elements (single parens are grouping).
    TupleLit {
        elems: Vec<Expr>,
        span: Span,
    },
    Pipeline {
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    /// `Point { x = 1, y = 2 }`
    StructLit {
        name: Sym,
        fields: Vec<(Sym, Expr)>,
        span: Span,
    },
    /// `p with { x = 10 }`
    With {
        base: Box<Expr>,
        fields: Vec<(Sym, Expr)>,
        span: Span,
    },
    /// `scope { body }` / `scope(schedulerExpr) { body }` — structured concurrency.
    Scope {
        scheduler: Option<Box<Expr>>,
        body: Box<Expr>,
        span: Span,
    },
    /// `spawn { body }` — start a task; block tail is the task result.
    Spawn {
        body: Box<Expr>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum InterpPart {
    Lit(Sym),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

/// Arm of subjectless `match { }`. `cond: None` means `_` (else).
#[derive(Debug, Clone)]
pub struct MatchCondArm {
    pub cond: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard(Span),
    Int(i64, Span),
    Float(f64, Span),
    /// `true` / `false` constant patterns (DESIGN § match 常量模式).
    Bool(bool, Span),
    Char(char, Span),
    String(Sym, Span),
    Ident(Sym, Span),
    Variant {
        name: Sym,
        args: Vec<Pattern>,
        span: Span,
    },
    /// `Point { x, y }` / `Point { x, y = 0 }`
    Struct {
        name: Sym,
        fields: Vec<(Sym, Pattern)>,
        span: Span,
    },
    Tuple {
        elems: Vec<Pattern>,
        span: Span,
    },
    List {
        elems: Vec<Pattern>,
        rest: Option<Sym>,
        span: Span,
    },
    Or(Vec<Pattern>, Span),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// `val x = e` / `val x: Int = e` or irrefutable destructure.
    Val {
        pat: Pattern,
        /// Ascription only valid for simple `Ident` binders.
        ty: Option<String>,
        expr: Expr,
        span: Span,
    },
    /// `var x = e` / `var x: Int = e` (name binding only; destructure not allowed on `var`).
    Var {
        name: Sym,
        ty: Option<String>,
        expr: Expr,
        span: Span,
    },
    Assign {
        name: Sym,
        expr: Expr,
        span: Span,
    },
    Expr(Expr),
    ForIn {
        binding: ForBinding,
        iter: Expr,
        body: Expr,
        span: Span,
    },
    ForCond {
        cond: Expr,
        body: Expr,
        span: Span,
    },
    Break(Span),
    Continue(Span),
}

/// `for x in …` or `for (k, v) in …` (Map pairs / List of pairs).
#[derive(Debug, Clone, PartialEq)]
pub enum ForBinding {
    Name(Sym),
    Pair(Sym, Sym),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Bool(_, s)
            | Expr::String(_, s)
            | Expr::Char(_, s)
            | Expr::Ident(_, s) => *s,
            Expr::Interp { span, .. }
            | Expr::Block { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Call { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Unary { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::MatchCond { span, .. }
            | Expr::Return { span, .. }
            | Expr::Alt { span, .. }
            | Expr::Field { span, .. }
            | Expr::ListLit { span, .. }
            | Expr::Pipeline { span, .. }
            | Expr::StructLit { span, .. }
            | Expr::With { span, .. }
            | Expr::TupleLit { span, .. }
            | Expr::Scope { span, .. }
            | Expr::Spawn { span, .. } => *span,
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
        };
        write!(f, "{s}")
    }
}

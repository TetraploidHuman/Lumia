//! Hand-written lexer and recursive-descent parser for Lumia.
//! Spans are preserved for diagnostics and LSP.

mod diag;
mod pretty;
mod lexer;
mod parser;
mod span;
mod stamp;
mod token;

pub use diag::{byte_to_line_col, format_diagnostic, line_starts};
pub use pretty::format_module_src;
pub use lexer::Lexer;
pub use parser::{parse_expr_str, parse_module, ParseError};
pub use span::{BytePos, Span};
pub use stamp::stamp_module;
pub use token::{StringPart, Token};

use std::fmt;

/// Parsed module AST (syntax level).
#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub span: Span,
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct Import {
    pub path: Vec<String>,
    pub names: ImportNames,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ImportNames {
    /// `import a.b`
    Single(String),
    /// `import a.{b, c}`
    Selective(Vec<String>),
    /// `import a.*`
    All,
}

#[derive(Debug, Clone)]
pub enum Item {
    Val(ValItem),
    Type(TypeItem),
    /// `foreign "C" fn name(x: Int) -> Int`
    Foreign(ForeignItem),
}

#[derive(Debug, Clone)]
pub struct ForeignItem {
    pub abi: String,
    pub name: String,
    pub params: Vec<(String, String)>,
    pub ret: String,
    /// `foreign "C" pure fn` — typed as Pure (math-like libc).
    pub is_pure: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ValItem {
    pub name: String,
    pub params: Option<Vec<String>>,
    pub body: Expr,
    pub span: Span,
    /// `priv val` — not re-exported via import.
    pub is_priv: bool,
}

#[derive(Debug, Clone)]
pub struct TypeItem {
    pub name: String,
    pub kind: TypeKind,
    pub span: Span,
    pub is_priv: bool,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    /// Product: `val` fields
    Product(Vec<String>),
    /// Sum: variants
    Sum(Vec<Variant>),
}

#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub fields: VariantFields,
}

#[derive(Debug, Clone)]
pub enum VariantFields {
    Unit,
    Positional(usize),
    Named(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    Bool(bool, Span),
    String(String, Span),
    /// Desugared interpolation: `"a${x}b"` → parts lit/expr alternating.
    Interp {
        parts: Vec<InterpPart>,
        span: Span,
    },
    Char(char, Span),
    Ident(String, Span),
    /// Block: statements + optional trailing expr value
    Block {
        stmts: Vec<Stmt>,
        tail: Option<Box<Expr>>,
        span: Span,
    },
    /// `{ a, b -> body }` or `{ body }`
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
    Field {
        base: Box<Expr>,
        field: String,
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
        name: String,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    /// `p with { x = 10 }`
    With {
        base: Box<Expr>,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum InterpPart {
    Lit(String),
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
    String(String, Span),
    Ident(String, Span),
    Variant {
        name: String,
        args: Vec<Pattern>,
        span: Span,
    },
    /// `Point { x, y }` / `Point { x, y = 0 }`
    Struct {
        name: String,
        fields: Vec<(String, Pattern)>,
        span: Span,
    },
    Tuple {
        elems: Vec<Pattern>,
        span: Span,
    },
    List {
        elems: Vec<Pattern>,
        rest: Option<String>,
        span: Span,
    },
    Or(Vec<Pattern>, Span),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Val {
        name: String,
        expr: Expr,
        span: Span,
    },
    Var {
        name: String,
        expr: Expr,
        span: Span,
    },
    Assign {
        name: String,
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

/// `for x in …` or `for (k, v) in …` (Map pairs).
#[derive(Debug, Clone, PartialEq)]
pub enum ForBinding {
    Name(String),
    Pair(String, String),
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
            | Expr::Ident(_, s) => *s, Expr::Interp { span, .. }
            | Expr::Block { span, .. }
            | Expr::Lambda { span, .. }
            | Expr::Call { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Unary { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::MatchCond { span, .. }
            | Expr::Field { span, .. }
            | Expr::ListLit { span, .. }
            | Expr::Pipeline { span, .. }
            | Expr::StructLit { span, .. }
            | Expr::With { span, .. }
            | Expr::TupleLit { span, .. } => *span,
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

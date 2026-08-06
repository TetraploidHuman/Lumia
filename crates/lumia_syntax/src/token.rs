use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Fragment of an interpolated string literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StringPart {
    Lit(String),
    /// `$name`
    Ident(String),
    /// `${…}` raw source (re-parsed as an expression).
    ExprSrc(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literals
    Int(i64),
    Float(f64),
    String(String),
    /// String with `$ident` / `${…}` pieces (lexer already split).
    InterpString(Vec<StringPart>),
    Ident(String),
    /// Unicode scalar value
    Char(char),

    // Keywords
    Module,
    Import,
    Val,
    Var,
    Type,
    If,
    Else,
    Match,
    For,
    In,
    Break,
    Continue,
    And,
    Or,
    Not,
    True,
    False,
    Priv,
    As,
    Trait,
    Instance,
    Requires,
    With,
    Effect,

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    DotDot,
    DotDotEq,
    Colon,
    ColonColon,
    Semi,
    Arrow,     // ->
    FatArrow,  // => (rejected by parser; reserved)
    PipePipe,  // >>
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Hash,
    Underscore,
    Ellipsis, // .. in patterns as rest marker handled via DotDot

    Eof,
}

impl TokenKind {
    pub fn keyword(s: &str) -> Option<TokenKind> {
        Some(match s {
            "module" => TokenKind::Module,
            "import" => TokenKind::Import,
            "val" => TokenKind::Val,
            "var" => TokenKind::Var,
            "type" => TokenKind::Type,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "match" => TokenKind::Match,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "priv" => TokenKind::Priv,
            "as" => TokenKind::As,
            "trait" => TokenKind::Trait,
            "instance" => TokenKind::Instance,
            "requires" => TokenKind::Requires,
            "with" => TokenKind::With,
            "effect" => TokenKind::Effect,
            _ => return None,
        })
    }
}

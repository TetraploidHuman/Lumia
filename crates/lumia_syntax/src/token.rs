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
    /// `$name` — `abs_start` is the byte offset of `name` in the enclosing file.
    Ident { name: String, abs_start: u32 },
    /// `${…}` raw source (re-parsed as an expression).
    /// `abs_start` is the byte offset of `src` (content after `${`) in the enclosing file.
    ExprSrc { src: String, abs_start: u32 },
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
    Return,
    Alt,
    And,
    Or,
    Not,
    /// Infix map pair sugar `a to b` (DESIGN §3.5.2).
    To,
    True,
    False,
    Priv,
    As,
    Trait,
    Instance,
    Requires,
    With,
    Effect,
    Scope,
    Spawn,
    Foreign,

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
    /// Half-open range `a..<b` (Kotlin-style; desugars to `range`).
    DotDotLt,
    /// Legacy `a..=b` — lexed so the parser can emit a targeted removal error.
    DotDotEq,
    Colon,
    ColonColon,
    Semi,
    Arrow,    // ->
    // (no FatArrow: `=>` lexes as Error)
    PipePipe, // >>
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

    /// Lexical error (e.g. integer literal overflow); parser turns this into a diagnostic.
    Error(String),

    Eof,
}

impl TokenKind {
    /// Lexer keyword spellings — single source of truth for editors / LSP.
    ///
    /// `pure` / `fn` are **not** keywords (`foreign` decls parse them as idents);
    /// editors may still highlight them as surface soft keywords.
    pub const KEYWORDS: &[&str] = &[
        "module", "import", "val", "var", "type", "if", "else", "match", "for", "in", "break",
        "continue", "return", "alt", "and", "or", "not", "to", "true", "false", "priv", "as",
        "trait", "instance", "requires", "with", "effect", "scope", "spawn", "foreign",
    ];

    /// Highlight-only spellings used in `foreign … fn` / `pure fn` surface syntax.
    pub const SURFACE_SOFT: &[&str] = &["pure", "fn"];

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
            "return" => TokenKind::Return,
            "alt" => TokenKind::Alt,
            "and" => TokenKind::And,
            "or" => TokenKind::Or,
            "not" => TokenKind::Not,
            "to" => TokenKind::To,
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            "priv" => TokenKind::Priv,
            "as" => TokenKind::As,
            "trait" => TokenKind::Trait,
            "instance" => TokenKind::Instance,
            "requires" => TokenKind::Requires,
            "with" => TokenKind::With,
            "effect" => TokenKind::Effect,
            "scope" => TokenKind::Scope,
            "spawn" => TokenKind::Spawn,
            "foreign" => TokenKind::Foreign,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod keyword_truth_tests {
    use super::TokenKind;

    #[test]
    fn keywords_const_matches_keyword_fn() {
        for &s in TokenKind::KEYWORDS {
            assert!(
                TokenKind::keyword(s).is_some(),
                "KEYWORDS entry `{s}` missing from keyword()"
            );
        }
        for &s in TokenKind::SURFACE_SOFT {
            assert!(
                TokenKind::keyword(s).is_none(),
                "SURFACE_SOFT `{s}` must not be a real keyword"
            );
        }
    }
}

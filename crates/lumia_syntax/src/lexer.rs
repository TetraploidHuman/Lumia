use crate::span::Span;
use crate::token::{Token, TokenKind};

pub struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// Peek the next `n` token kinds without advancing this lexer.
    pub fn peek_kinds(&self, n: usize) -> Vec<TokenKind> {
        let mut tmp = Lexer {
            src: self.src,
            bytes: self.bytes,
            pos: self.pos,
        };
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(tmp.next_token().kind);
        }
        out
    }

    pub fn next_token(&mut self) -> Token {
        self.skip_trivia();
        let start = self.pos as u32;
        if self.pos >= self.bytes.len() {
            return Token {
                kind: TokenKind::Eof,
                span: Span::new(start, start),
            };
        }
        let b = self.bytes[self.pos];
        let kind = match b {
            b'(' => {
                self.pos += 1;
                TokenKind::LParen
            }
            b')' => {
                self.pos += 1;
                TokenKind::RParen
            }
            b'{' => {
                self.pos += 1;
                TokenKind::LBrace
            }
            b'}' => {
                self.pos += 1;
                TokenKind::RBrace
            }
            b'[' => {
                self.pos += 1;
                TokenKind::LBracket
            }
            b']' => {
                self.pos += 1;
                TokenKind::RBracket
            }
            b',' => {
                self.pos += 1;
                TokenKind::Comma
            }
            b';' => {
                self.pos += 1;
                TokenKind::Semi
            }
            b'#' => {
                self.pos += 1;
                TokenKind::Hash
            }
            b'+' => {
                self.pos += 1;
                TokenKind::Plus
            }
            b'*' => {
                self.pos += 1;
                TokenKind::Star
            }
            b'%' => {
                self.pos += 1;
                TokenKind::Percent
            }
            b'/' => {
                // comments already skipped; this is division
                self.pos += 1;
                TokenKind::Slash
            }
            b'-' => {
                self.pos += 1;
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            b'=' => {
                self.pos += 1;
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    TokenKind::EqEq
                } else if self.peek() == Some(b'>') {
                    self.pos += 1;
                    TokenKind::FatArrow
                } else {
                    TokenKind::Eq
                }
            }
            b'!' => {
                self.pos += 1;
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    TokenKind::Ne
                } else {
                    // bare ! not used; treat as ident error via unknown
                    TokenKind::Ident("!".into())
                }
            }
            b'<' => {
                self.pos += 1;
                if self.peek() == Some(b'=') {
                    self.pos += 1;
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            b'>' => {
                self.pos += 1;
                if self.peek() == Some(b'>') {
                    self.pos += 1;
                    TokenKind::PipePipe
                } else if self.peek() == Some(b'=') {
                    self.pos += 1;
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            b'.' => {
                self.pos += 1;
                if self.peek() == Some(b'.') {
                    self.pos += 1;
                    if self.peek() == Some(b'=') {
                        self.pos += 1;
                        TokenKind::DotDotEq
                    } else {
                        TokenKind::DotDot
                    }
                } else {
                    TokenKind::Dot
                }
            }
            b':' => {
                self.pos += 1;
                if self.peek() == Some(b':') {
                    self.pos += 1;
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }
            b'"' => self.lex_string(),
            b'\'' => self.lex_char(),
            b'0'..=b'9' => self.lex_number(),
            b'_' => {
                self.pos += 1;
                if self.peek().is_some_and(|c| is_ident_continue(c)) {
                    self.pos -= 1;
                    self.lex_ident()
                } else {
                    TokenKind::Underscore
                }
            }
            b'a'..=b'z' | b'A'..=b'Z' => self.lex_ident(),
            _ => {
                self.pos += 1;
                TokenKind::Error(format!("invalid character U+{b:02X}"))
            }
        };
        let end = self.pos as u32;
        Token {
            kind,
            span: Span::new(start, end),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_trivia(&mut self) {
        loop {
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos] == b'/'
                && self.bytes[self.pos + 1] == b'/'
            {
                self.pos += 2;
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            if self.pos + 1 < self.bytes.len()
                && self.bytes[self.pos] == b'/'
                && self.bytes[self.pos + 1] == b'*'
            {
                self.pos += 2;
                while self.pos + 1 < self.bytes.len()
                    && !(self.bytes[self.pos] == b'*' && self.bytes[self.pos + 1] == b'/')
                {
                    self.pos += 1;
                }
                if self.pos + 1 < self.bytes.len() {
                    self.pos += 2;
                }
                continue;
            }
            break;
        }
    }

    fn lex_ident(&mut self) -> TokenKind {
        let start = self.pos;
        self.pos += 1;
        while self.pos < self.bytes.len() && is_ident_continue(self.bytes[self.pos]) {
            self.pos += 1;
        }
        let s = &self.src[start..self.pos];
        if let Some(kw) = TokenKind::keyword(s) {
            kw
        } else {
            TokenKind::Ident(s.to_string())
        }
    }

    fn lex_number(&mut self) -> TokenKind {
        let start = self.pos;
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'_')
        {
            self.pos += 1;
        }
        if self.peek() == Some(b'.')
            && self
                .bytes
                .get(self.pos + 1)
                .is_some_and(|c| c.is_ascii_digit())
        {
            self.pos += 1;
            while self.pos < self.bytes.len()
                && (self.bytes[self.pos].is_ascii_digit() || self.bytes[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let raw: String = self.src[start..self.pos].chars().filter(|c| *c != '_').collect();
            match raw.parse::<f64>() {
                Ok(n) if n.is_finite() => TokenKind::Float(n),
                Ok(_) => TokenKind::Error(format!("float literal `{raw}` is not finite")),
                Err(_) => TokenKind::Error(format!("invalid float literal `{raw}`")),
            }
        } else {
            let raw: String = self.src[start..self.pos].chars().filter(|c| *c != '_').collect();
            match raw.parse::<i64>() {
                Ok(n) => TokenKind::Int(n),
                Err(_) => TokenKind::Error(format!(
                    "integer literal `{raw}` is out of range for Int (i64)"
                )),
            }
        }
    }

    fn lex_string(&mut self) -> TokenKind {
        self.pos += 1; // opening "
        let mut parts: Vec<crate::token::StringPart> = Vec::new();
        let mut lit = String::new();
        let flush_lit = |parts: &mut Vec<crate::token::StringPart>, lit: &mut String| {
            if !lit.is_empty() {
                parts.push(crate::token::StringPart::Lit(std::mem::take(lit)));
            }
        };
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            if c == b'"' {
                self.pos += 1;
                break;
            }
            if c == b'\\' {
                self.pos += 1;
                if self.pos >= self.bytes.len() {
                    break;
                }
                match self.bytes[self.pos] {
                    b'n' => lit.push('\n'),
                    b't' => lit.push('\t'),
                    b'r' => lit.push('\r'),
                    b'\\' => lit.push('\\'),
                    b'"' => lit.push('"'),
                    b'$' => lit.push('$'),
                    other => lit.push(other as char),
                }
                self.pos += 1;
                continue;
            }
            if c == b'$' {
                let next = self.bytes.get(self.pos + 1).copied();
                if next == Some(b'{') {
                    flush_lit(&mut parts, &mut lit);
                    self.pos += 2; // ${
                    let start = self.pos;
                    let mut depth = 1i32;
                    while self.pos < self.bytes.len() && depth > 0 {
                        let ch = self.bytes[self.pos];
                        if ch == b'{' {
                            depth += 1;
                        } else if ch == b'}' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        } else if ch == b'"' {
                            // Skip nested string literal inside `${…}`.
                            self.pos += 1;
                            while self.pos < self.bytes.len() {
                                let sc = self.bytes[self.pos];
                                if sc == b'\\' {
                                    self.pos = self.pos.saturating_add(2);
                                    continue;
                                }
                                self.pos += 1;
                                if sc == b'"' {
                                    break;
                                }
                            }
                            continue;
                        } else if ch == b'\'' {
                            // Skip char literal so `'}'` does not close `${…}`.
                            self.pos += 1;
                            while self.pos < self.bytes.len() {
                                let sc = self.bytes[self.pos];
                                if sc == b'\\' {
                                    self.pos = self.pos.saturating_add(2);
                                    continue;
                                }
                                self.pos += 1;
                                if sc == b'\'' {
                                    break;
                                }
                            }
                            continue;
                        }
                        self.pos += 1;
                    }
                    let inner = self.src[start..self.pos.min(self.bytes.len())].to_string();
                    if self.pos < self.bytes.len() && self.bytes[self.pos] == b'}' {
                        self.pos += 1;
                    }
                    parts.push(crate::token::StringPart::ExprSrc(inner));
                    continue;
                }
                if next.is_some_and(|b| b.is_ascii_alphabetic() || b == b'_') {
                    flush_lit(&mut parts, &mut lit);
                    self.pos += 1;
                    let start = self.pos;
                    while self.pos < self.bytes.len() && is_ident_continue(self.bytes[self.pos]) {
                        self.pos += 1;
                    }
                    let name = self.src[start..self.pos].to_string();
                    parts.push(crate::token::StringPart::Ident(name));
                    continue;
                }
            }
            // UTF-8 safe: push one char
            let ch = self.src[self.pos..].chars().next().unwrap_or('\0');
            lit.push(ch);
            self.pos += ch.len_utf8();
        }
        flush_lit(&mut parts, &mut lit);
        if parts.is_empty() {
            return TokenKind::String(String::new());
        }
        if parts.len() == 1 {
            if let crate::token::StringPart::Lit(s) = &parts[0] {
                return TokenKind::String(s.clone());
            }
        }
        TokenKind::InterpString(parts)
    }

    fn lex_char(&mut self) -> TokenKind {
        self.pos += 1; // opening '
        if self.pos >= self.bytes.len() {
            return TokenKind::Error("unterminated character literal".into());
        }
        let ch = if self.bytes[self.pos] == b'\\' {
            self.pos += 1;
            if self.pos >= self.bytes.len() {
                return TokenKind::Error("unterminated character escape".into());
            }
            let esc = self.bytes[self.pos];
            self.pos += 1;
            match esc {
                b'n' => '\n',
                b't' => '\t',
                b'r' => '\r',
                b'\\' => '\\',
                b'\'' => '\'',
                b'0' => '\0',
                other => other as char,
            }
        } else {
            // Decode one UTF-8 scalar from remaining bytes
            let rest = &self.src[self.pos..];
            let Some(ch) = rest.chars().next() else {
                return TokenKind::Error("unterminated character literal".into());
            };
            self.pos += ch.len_utf8();
            ch
        };
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'\'' {
            self.pos += 1;
            TokenKind::Char(ch)
        } else {
            TokenKind::Error("unterminated character literal".into())
        }
    }
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_keywords_and_ops() {
        let mut lx = Lexer::new("val x = 1 + 2 >> f");
        let mut kinds = vec![];
        loop {
            let t = lx.next_token();
            let done = t.kind == TokenKind::Eof;
            kinds.push(t.kind);
            if done {
                break;
            }
        }
        assert!(matches!(kinds[0], TokenKind::Val));
        assert!(matches!(kinds[3], TokenKind::Int(1)));
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::PipePipe)));
    }

    #[test]
    fn interp_skips_char_literal_braces() {
        let mut lx = Lexer::new(r#""x${'}'}y""#);
        let t = lx.next_token();
        match t.kind {
            TokenKind::InterpString(parts) => {
                assert!(
                    parts.iter().any(|p| matches!(
                        p,
                        crate::token::StringPart::ExprSrc(s) if s.contains('\'')
                    )),
                    "char literal must stay inside ExprSrc, got {parts:?}"
                );
                assert!(
                    !parts.iter().any(|p| matches!(
                        p,
                        crate::token::StringPart::Lit(s) if s.contains('}')
                    )),
                    "closing brace of char must not end interpolation early: {parts:?}"
                );
            }
            other => panic!("expected InterpString, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_char_is_error() {
        let mut lx = Lexer::new("'");
        let t = lx.next_token();
        assert!(
            matches!(t.kind, TokenKind::Error(ref m) if m.contains("unterminated")),
            "got {:?}",
            t.kind
        );
    }

    #[test]
    fn invalid_byte_is_error_not_fake_ident() {
        let mut lx = Lexer::new("$");
        let t = lx.next_token();
        assert!(
            matches!(t.kind, TokenKind::Error(_)),
            "got {:?}",
            t.kind
        );
    }
}

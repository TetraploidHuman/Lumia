use crate::escape::{unescape_char_byte, unescape_string_byte};
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

    /// Advance one UTF-8 scalar at `pos` (at least one byte if the slice is invalid).
    fn advance_scalar(&mut self) {
        if self.pos >= self.bytes.len() {
            return;
        }
        let ch = self.src[self.pos..].chars().next().unwrap_or('\0');
        self.pos += ch.len_utf8().max(1);
    }

    /// Skip a nested `"…"` / `'…'` literal (caller already consumed the opening quote).
    /// Escape sequences advance by scalar so multi-byte content cannot desync `pos`.
    fn skip_quoted_literal(&mut self, quote: u8) {
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b == b'\\' {
                self.pos += 1;
                self.advance_scalar();
                continue;
            }
            if b == quote {
                self.pos += 1;
                return;
            }
            self.advance_scalar();
        }
    }

    /// Peek the next `n` token kinds without advancing this lexer.
    ///
    /// Allocates `Ident`/`String` payloads like a real lex; prefer
    /// [`Self::peek_ident_eq`] when only a structural look-ahead is needed.
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

    /// True if the next two tokens are `Ident` then `=` (struct field), without
    /// cloning Ident strings or re-running a full speculative lexer.
    pub fn peek_ident_eq(&self) -> bool {
        let mut pos = skip_trivia_at(self.bytes, self.pos);
        let start = pos;
        if pos < self.bytes.len() && self.bytes[pos] == b'_' {
            pos += 1;
            if pos >= self.bytes.len() || !is_ident_continue(self.bytes[pos]) {
                return false; // bare `_` → Underscore, not Ident
            }
            while pos < self.bytes.len() && is_ident_continue(self.bytes[pos]) {
                pos += 1;
            }
        } else if pos < self.bytes.len() && self.bytes[pos].is_ascii_alphabetic() {
            pos += 1;
            while pos < self.bytes.len() && is_ident_continue(self.bytes[pos]) {
                pos += 1;
            }
        } else {
            return false;
        }
        let word = &self.src[start..pos];
        // Hard keywords are their own token kinds — not `Ident`.
        if TokenKind::keyword(word).is_some() {
            return false;
        }
        pos = skip_trivia_at(self.bytes, pos);
        if self.bytes.get(pos) != Some(&b'=') {
            return false;
        }
        match self.bytes.get(pos + 1).copied() {
            Some(b'=') | Some(b'>') => false, // `==` / `=>`
            _ => true,
        }
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
                    TokenKind::Error(
                        "`=>` is not a Lumia token (use `{ … }` lambdas / `if` match arms)".into(),
                    )
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
                    TokenKind::GtGt
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
                    if self.peek() == Some(b'<') {
                        self.pos += 1;
                        TokenKind::DotDotLt
                    } else if self.peek() == Some(b'=') {
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
                if self.peek().is_some_and(is_ident_continue) {
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
            let raw: String = self.src[start..self.pos]
                .chars()
                .filter(|c| *c != '_')
                .collect();
            match raw.parse::<f64>() {
                Ok(n) if n.is_finite() => TokenKind::Float(n),
                Ok(_) => TokenKind::Error(format!("float literal `{raw}` is not finite")),
                Err(_) => TokenKind::Error(format!("invalid float literal `{raw}`")),
            }
        } else {
            let raw: String = self.src[start..self.pos]
                .chars()
                .filter(|c| *c != '_')
                .collect();
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
                match unescape_string_byte(self.bytes[self.pos]) {
                    Some(ch) => lit.push(ch),
                    None => lit.push(self.bytes[self.pos] as char),
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
                            self.skip_quoted_literal(b'"');
                            continue;
                        } else if ch == b'\'' {
                            // Skip char literal so `'}'` does not close `${…}`.
                            self.pos += 1;
                            self.skip_quoted_literal(b'\'');
                            continue;
                        }
                        self.pos += 1;
                    }
                    let inner = self.src[start..self.pos.min(self.bytes.len())].to_string();
                    if self.pos < self.bytes.len() && self.bytes[self.pos] == b'}' {
                        self.pos += 1;
                    }
                    parts.push(crate::token::StringPart::ExprSrc {
                        src: inner,
                        abs_start: start as u32,
                    });
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
                    parts.push(crate::token::StringPart::Ident {
                        name,
                        abs_start: start as u32,
                    });
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
            match unescape_char_byte(esc) {
                Some(ch) => ch,
                None => esc as char,
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

/// Skip whitespace and `//` / `/* */` comments starting at `pos` (peek helpers).
fn skip_trivia_at(bytes: &[u8], mut pos: usize) -> usize {
    loop {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos + 1 < bytes.len() && bytes[pos] == b'/' && bytes[pos + 1] == b'/' {
            pos += 2;
            while pos < bytes.len() && bytes[pos] != b'\n' {
                pos += 1;
            }
            continue;
        }
        if pos + 1 < bytes.len() && bytes[pos] == b'/' && bytes[pos + 1] == b'*' {
            pos += 2;
            while pos + 1 < bytes.len() && !(bytes[pos] == b'*' && bytes[pos + 1] == b'/') {
                pos += 1;
            }
            if pos + 1 < bytes.len() {
                pos += 2;
            }
            continue;
        }
        break;
    }
    pos
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
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::GtGt)));
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
                        crate::token::StringPart::ExprSrc { src, .. } if src.contains('\'')
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
    fn interp_nested_string_escape_advances_by_scalar() {
        // `\中` is two UTF-8 bytes after `\`; old +2 skip could desync and eat the closer.
        let mut lx = Lexer::new("\"a${\"\\中\"}b\"");
        let t = lx.next_token();
        match t.kind {
            TokenKind::InterpString(parts) => {
                assert!(
                    parts.iter().any(|p| matches!(
                        p,
                        crate::token::StringPart::ExprSrc { src, .. } if src.contains('中')
                    )),
                    "nested string must stay in ExprSrc: {parts:?}"
                );
                assert!(
                    parts.iter().any(|p| matches!(
                        p,
                        crate::token::StringPart::Lit(s) if s == "b"
                    )),
                    "outer literal after }} must remain: {parts:?}"
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
    fn to_is_hard_keyword() {
        let mut lx = Lexer::new("1 to 2");
        let _ = lx.next_token(); // 1
        let t = lx.next_token();
        assert!(matches!(t.kind, TokenKind::To), "got {:?}", t.kind);
    }

    #[test]
    fn peek_ident_eq_matches_peek_kinds_and_skips_keywords() {
        let cases = [
            ("x = 1", true),
            ("_y = 1", true),
            ("  /* c */ x = 1", true),
            ("val = 1", false), // keyword, not Ident
            ("_ = 1", false),   // Underscore
            ("x == 1", false),
            ("x => 1", false),
            ("1 = 1", false),
            ("{ x -> x }", false),
        ];
        for (src, want) in cases {
            let lx = Lexer::new(src);
            assert_eq!(lx.peek_ident_eq(), want, "peek_ident_eq({src:?})");
            let kinds = lx.peek_kinds(2);
            let via_kinds = matches!(
                (kinds.first(), kinds.get(1)),
                (Some(TokenKind::Ident(_)), Some(TokenKind::Eq))
            );
            assert_eq!(via_kinds, want, "peek_kinds parity for {src:?}: {kinds:?}");
        }
    }

    #[test]
    fn invalid_byte_is_error_not_fake_ident() {
        let mut lx = Lexer::new("$");
        let t = lx.next_token();
        assert!(matches!(t.kind, TokenKind::Error(_)), "got {:?}", t.kind);
    }

    #[test]
    fn lex_kotlin_style_ranges() {
        let mut lx = Lexer::new("1..5 1..<5 1..=5");
        let mut kinds = vec![];
        loop {
            let t = lx.next_token();
            let done = t.kind == TokenKind::Eof;
            kinds.push(t.kind);
            if done {
                break;
            }
        }
        assert!(
            matches!(kinds[1], TokenKind::DotDot),
            "inclusive .., got {:?}",
            kinds[1]
        );
        assert!(
            matches!(kinds[4], TokenKind::DotDotLt),
            "half-open ..<, got {:?}",
            kinds[4]
        );
        assert!(
            matches!(kinds[7], TokenKind::DotDotEq),
            "legacy ..= still lexed, got {:?}",
            kinds[7]
        );
    }
}

//! Structured diagnostic kind (shared by CLI check + LSP).

use lumia_syntax::Span;

/// Compiler phase that produced a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Parse,
    Lower,
    Type,
    /// Load / analysis failure without a phase prefix.
    Other,
}

impl DiagnosticKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parse => "parse",
            Self::Lower => "lower",
            Self::Type => "type",
            Self::Other => "error",
        }
    }

    /// LSP `Diagnostic.severity` (Error=1, Warning=2, Information=3, Hint=4).
    ///
    /// All current kinds are hard failures; wire new soft kinds here when added.
    pub fn lsp_severity(self) -> u8 {
        match self {
            Self::Parse | Self::Lower | Self::Type | Self::Other => 1,
        }
    }

    /// LSP `Diagnostic.code` (omit for [`Self::Other`]).
    pub fn lsp_code(self) -> Option<&'static str> {
        match self {
            Self::Other => None,
            k => Some(k.as_str()),
        }
    }

    /// Best-effort parse of a legacy `kind: …` message prefix.
    pub fn from_message_prefix(msg: &str) -> Self {
        let lower = msg.to_ascii_lowercase();
        if lower.starts_with("parse:") {
            Self::Parse
        } else if lower.starts_with("lower:") {
            Self::Lower
        } else if lower.starts_with("type:") {
            Self::Type
        } else {
            Self::Other
        }
    }

    /// Display text: `kind: message`, without doubling an existing prefix.
    pub fn format_message(self, msg: &str) -> String {
        match self {
            Self::Other => msg.to_string(),
            k => {
                let prefix = format!("{}:", k.as_str());
                if msg.len() >= prefix.len()
                    && msg.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
                {
                    msg.to_string()
                } else {
                    format!("{prefix} {msg}")
                }
            }
        }
    }
}

/// One recoverable / published diagnostic.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    pub kind: DiagnosticKind,
    /// Human text without a required `kind:` prefix.
    pub message: String,
}

impl Diagnostic {
    pub fn new(span: Span, kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            span,
            kind,
            message: message.into(),
        }
    }

    pub fn display_message(&self) -> String {
        self.kind.format_message(&self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::DiagnosticKind;

    #[test]
    fn format_message_avoids_double_prefix() {
        assert_eq!(
            DiagnosticKind::Type.format_message("mismatch"),
            "type: mismatch"
        );
        assert_eq!(
            DiagnosticKind::Type.format_message("type: mismatch"),
            "type: mismatch"
        );
        assert_eq!(
            DiagnosticKind::Type.format_message("TYPE: mismatch"),
            "TYPE: mismatch"
        );
    }

    #[test]
    fn from_prefix() {
        assert_eq!(
            DiagnosticKind::from_message_prefix("parse: eof"),
            DiagnosticKind::Parse
        );
        assert_eq!(
            DiagnosticKind::from_message_prefix("nope"),
            DiagnosticKind::Other
        );
    }
}

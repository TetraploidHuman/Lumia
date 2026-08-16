//! Pattern formatting.

use super::escape_str;
use crate::Pattern;

pub(crate) fn format_pat(out: &mut String, p: &Pattern) {
    match p {
        Pattern::Wildcard(_) => out.push('_'),
        Pattern::Int(n, _) => out.push_str(&n.to_string()),
        Pattern::Float(n, _) => out.push_str(&n.to_string()),
        Pattern::Bool(b, _) => out.push_str(if *b { "true" } else { "false" }),
        Pattern::Char(c, _) => {
            out.push('\'');
            match *c {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                other => out.push(other),
            }
            out.push('\'');
        }
        Pattern::String(s, _) => {
            out.push('"');
            out.push_str(&escape_str(s));
            out.push('"');
        }
        Pattern::Ident(n, _) => out.push_str(n),
        Pattern::Variant { name, args, .. } => {
            out.push_str(name);
            if !args.is_empty() {
                out.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_pat(out, a);
                }
                out.push(')');
            }
        }
        Pattern::Tuple { elems, .. } => {
            out.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_pat(out, e);
            }
            out.push(')');
        }
        Pattern::List { elems, rest, .. } => {
            out.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_pat(out, e);
            }
            if let Some(r) = rest {
                if !elems.is_empty() {
                    out.push_str(", ");
                }
                out.push_str("..");
                out.push_str(r);
            }
            out.push(']');
        }
        Pattern::Struct { name, fields, .. } => {
            out.push_str(name);
            out.push_str(" { ");
            for (i, (f, p)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(f);
                out.push_str(" = ");
                format_pat(out, p);
            }
            out.push_str(" }");
        }
        Pattern::Or(ps, _) => {
            for (i, p) in ps.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                format_pat(out, p);
            }
        }
    }
}

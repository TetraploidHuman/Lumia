//! textDocument/signatureHelp.
//!
//! Contract: use `Analysis.typed.fun_types` for stable, IDE-friendly signatures.

use super::cursor::pos_to_byte;
use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_ty::{display_type, Type};
use serde_json::{json, Value};

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Find the opening `(` of the innermost *enclosing* call at `byte`.
///
/// This is a lightweight src-scan (not a full parser): it only matches `(...)`
/// call syntax and ignores parentheses inside strings/comments.
fn find_enclosing_call_open_paren(src: &str, byte: usize) -> Option<usize> {
    if src.is_empty() {
        return None;
    }
    let bytes = src.as_bytes();
    let mut depth: i32 = 0;
    for i in (0..byte.min(bytes.len())).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Extract a function name right before `(`.
///
/// Accepts only ascii `[A-Za-z0-9_]+` (no module qualifiers / method receivers).
fn fun_name_before_paren(src: &str, open_paren: usize) -> Option<String> {
    let bytes = src.as_bytes();
    if open_paren == 0 || open_paren > bytes.len() {
        return None;
    }
    let mut end = open_paren;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 || !is_ident_byte(bytes[end - 1]) {
        return None;
    }
    let mut start = end;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    if start >= end {
        return None;
    }
    std::str::from_utf8(&bytes[start..end]).ok().map(|s| s.to_string())
}

fn active_parameter_index(src: &str, open_paren: usize, byte: usize) -> usize {
    let bytes = src.as_bytes();
    let mut depth: i32 = 0;
    let mut commas: usize = 0;
    let upto = byte.min(bytes.len());
    for i in (open_paren + 1)..upto {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            b',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    commas
}

pub(crate) fn signature_help_for_analysis(a: &Analysis, line: u32, character: u32) -> Value {
    let byte = pos_to_byte(&a.src, line, character) as usize;
    let src = &a.src;
    let Some(open_paren) = find_enclosing_call_open_paren(src, byte) else {
        return Value::Null;
    };
    let Some(fun_name) = fun_name_before_paren(src, open_paren) else {
        return Value::Null;
    };

    let Some(fun_ty) = a.typed.fun_types.get(&fun_name) else {
        return Value::Null;
    };
    let num_vars: Vec<u32> = a
        .typed
        .fun_schemes
        .get(&fun_name)
        .map(|s| s.num_vars.clone())
        .unwrap_or_default();

    let active_param = active_parameter_index(src, open_paren, byte);
    match fun_ty {
        Type::Fun(params, _ret, _eff) => {
            let grounded = display_type(fun_ty, &num_vars);
            // display_type already renders `(A, B) -> R` so we just prefix name.
            let label = format!("{fun_name}{grounded}");

            let parameters: Vec<Value> = params
                .iter()
                .map(|p| json!({ "label": display_type(p, &num_vars) }))
                .collect();

            let active_parameter = if params.is_empty() {
                0
            } else {
                active_param.min(params.len().saturating_sub(1))
            };

            json!({
                "signatures": [
                    {
                        "label": label,
                        "parameters": parameters,
                    }
                ],
                "activeSignature": 0,
                "activeParameter": active_parameter,
            })
        }
        _ => Value::Null,
    }
}

pub(super) fn on_signature_help(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(Value::Null);
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    if uri.is_empty() {
        return Ok(Value::Null);
    }
    let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as u32;

    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(Value::Null);
    };
    let Some(a) = state.analysis.get(uri) else {
        return Ok(Value::Null);
    };
    Ok(signature_help_for_analysis(a, line, character))
}

#[cfg(test)]
mod tests {
    use super::signature_help_for_analysis;
    use crate::check::check_source;
    use crate::lsp::cursor::byte_to_position;
    use crate::lsp::state::Analysis;
    use crate::lsp::test_support::{imported_alias_analysis, with_encoding};
    use lumia_syntax::ColumnMetric;

    fn analysis(src: &str) -> Analysis {
        let typed = check_source(src, true).expect("typecheck");
        Analysis::from_typed(typed, src.to_string(), vec![], 0)
    }

    #[test]
    fn signature_help_add_call_active_param_0() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = r#"
module Demo
val add = { x, y -> x + y }
val main = { add(1, 2) }
"#;
            let a = analysis(src);

            let byte = src
                .find("add(1")
                .expect("add(")
                .saturating_add("add(".len()) as u32;
            let (line, character) = byte_to_position(src, byte);
            let out = signature_help_for_analysis(&a, line, character);

            let label = out["signatures"][0]["label"].as_str().unwrap_or("");
            assert_eq!(label, "add(Int, Int) -> Int");
            assert_eq!(out["activeParameter"].as_u64().unwrap(), 0);
            let p0 = out["signatures"][0]["parameters"][0]["label"].as_str().unwrap_or("");
            let p1 = out["signatures"][0]["parameters"][1]["label"].as_str().unwrap_or("");
            assert_eq!(p0, "Int");
            assert_eq!(p1, "Int");
        });
    }

    #[test]
    fn signature_help_add_call_active_param_1() {
        with_encoding(ColumnMetric::Utf16, || {
            let src = r#"
module Demo
val add = { x, y -> x + y }
val main = { add(1, 2) }
"#;
            let a = analysis(src);

            let byte = src.rfind('2').expect("arg 2") as u32;
            let (line, character) = byte_to_position(src, byte);
            let out = signature_help_for_analysis(&a, line, character);

            let label = out["signatures"][0]["label"].as_str().unwrap_or("");
            assert_eq!(label, "add(Int, Int) -> Int");
            assert_eq!(out["activeParameter"].as_u64().unwrap(), 1);
        });
    }

    #[test]
    fn signature_help_imported_alias_via_loader() {
        with_encoding(ColumnMetric::Utf16, || {
            let uri = "untitled:SigHelp-1";
            let a = imported_alias_analysis(uri);
            let src = crate::lsp::test_support::IMPORTED_ALIAS_SRC;

            let byte = src.find("log(1)").expect("log call").saturating_add(4) as u32; // at `1`
            let (line, character) = byte_to_position(src, byte);
            let out = signature_help_for_analysis(&a, line, character);

            assert!(
                !out.is_null(),
                "signature help on imported `log` must not be null"
            );
            let label = out["signatures"][0]["label"].as_str().unwrap_or("");
            assert!(label.starts_with("log("), "unexpected label: {label}");
            assert!(label.contains("-> Unit"), "unexpected label: {label}");
            assert!(label.contains(" / IO") || label.contains("/ IO"), "expected IO: {label}");
            assert_eq!(
                out["signatures"][0]["parameters"].as_array().unwrap().len(),
                1
            );
        });
    }
}


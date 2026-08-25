//! textDocument/inlayHint — binding / param / call-return type hints.

mod collect;
mod source;

use collect::collect_toplevel_hints;
use source::range_from_params;

use super::state::{state_lock, Analysis};
use anyhow::Result;
use serde_json::{json, Value};

pub(super) fn hints_for_analysis(a: &Analysis, range: Option<(u32, u32)>) -> Vec<Value> {
    let mut out = Vec::new();
    collect_toplevel_hints(a, &mut out, range);
    // Dedup identical position+label (toplevel Fun params may overlap nested walk).
    out.sort_by_key(|h| {
        (
            h["position"]["line"].as_u64().unwrap_or(0),
            h["position"]["character"].as_u64().unwrap_or(0),
            h["label"].as_str().unwrap_or("").to_string(),
        )
    });
    out.dedup_by(|a, b| a["position"] == b["position"] && a["label"] == b["label"]);
    out
}

pub(super) fn on_inlay_hint(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(json!([]));
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(json!([]));
    };
    let Some(a) = state.analysis.get(uri) else {
        return Ok(json!([]));
    };
    let range = range_from_params(&a.src, params);
    Ok(Value::Array(hints_for_analysis(a, range)))
}

#[cfg(test)]
mod tests {
    use super::super::state::Analysis;
    use super::hints_for_analysis;
    use crate::check::check_source;

    #[test]
    fn inlay_hints_binding_param_and_call() {
        let src = r#"
module Demo
import lumi.io.{println}
val add = { x, y ->
    x + y
}
val main = {
    val n = add(1, 2)
    println(n)
}
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis {
            typed,
            src: src.to_string(),
            files: vec![],
        };
        let hints = hints_for_analysis(&a, None);
        let labels: Vec<String> = hints
            .iter()
            .filter_map(|h| h["label"].as_str().map(|s| s.to_string()))
            .collect();
        // Top-level add should show Num-defaulted Int, not opaque type vars.
        assert!(
            labels.iter().any(|l| l.contains("(Int, Int) -> Int")),
            "expected (Int, Int) -> Int on add, got {labels:?}"
        );
        // Params x, y
        assert!(
            labels.iter().filter(|l| *l == "Int").count() >= 2,
            "expected param/local Int hints, got {labels:?}"
        );
    }

    #[test]
    fn inlay_paren_params_and_call_before_def() {
        // Call site appears *before* `val fun` — binding hint must not stick to the call.
        let src = r#"
module Demo
import lumi.io.{println}
val main = {
    println(fun(1, 2))
}
val fun(a, b) = {
    a + b
}
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis {
            typed,
            src: src.to_string(),
            files: vec![],
        };
        let hints = hints_for_analysis(&a, None);
        let by_line: Vec<(u64, String)> = hints
            .iter()
            .map(|h| {
                (
                    h["position"]["line"].as_u64().unwrap_or(0),
                    h["label"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        // `println(fun(1, 2))` is on 0-based line 4 — must not show Fun types there.
        let call_line: Vec<_> = by_line.iter().filter(|(l, _)| *l == 4).collect();
        assert!(
            call_line.iter().all(|(_, lab)| !lab.contains("->")),
            "call site must not show Fun type, got {call_line:?}"
        );
        assert!(
            by_line
                .iter()
                .any(|(_, lab)| lab.contains("(Int, Int) -> Int")),
            "expected fun binding type, got {by_line:?}"
        );
        assert!(
            by_line.iter().filter(|(_, lab)| *lab == "Int").count() >= 2,
            "expected a/b Int on val fun(a, b), got {by_line:?}"
        );
    }

    #[test]
    fn inlay_range_past_eof_still_returns_hints() {
        let src = r#"
module Demo
val add = { x, y -> x + y }
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis {
            typed,
            src: src.to_string(),
            files: vec![],
        };
        // Simulate VS Code asking for a huge visible range past the last line.
        let starts = lumi_syntax::line_starts(src);
        let bogus_end_line = starts.len() as u32 + 40;
        let params = serde_json::json!({
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": bogus_end_line, "character": 0 }
            }
        });
        let range = super::source::range_from_params(src, &params);
        let hints = hints_for_analysis(&a, range);
        assert!(
            !hints.is_empty(),
            "OOB end line must not wipe all inlay hints"
        );
    }

    #[test]
    fn inlay_skips_unit_and_fun_call_results() {
        let src = r#"
module Demo
import lumi.io.{println}
val id = { x -> x }
val main = {
    println(1)
    id
}
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis {
            typed,
            src: src.to_string(),
            files: vec![],
        };
        let hints = hints_for_analysis(&a, None);
        let labels: Vec<_> = hints.iter().filter_map(|h| h["label"].as_str()).collect();
        assert!(
            labels
                .iter()
                .all(|l| *l != "Unit" && *l != "( )" && !l.ends_with("Unit")),
            "Unit call results must not appear as hints, got {labels:?}"
        );
        // Bare `id` is not a Call — but if a Call returned Fun it must be filtered.
        // Nested lambda still gets param/return hints.
        assert!(
            labels
                .iter()
                .any(|l| l.contains("->") || *l == "Int" || l.starts_with(' ')),
            "expected nested lambda hints, got {labels:?}"
        );
    }

    #[test]
    fn inlay_nested_lambda_param_hints() {
        let src = r#"
module Demo
val outer = { x ->
    val inner = { y -> x + y }
    inner(1)
}
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis {
            typed,
            src: src.to_string(),
            files: vec![],
        };
        let hints = hints_for_analysis(&a, None);
        let labels: Vec<_> = hints
            .iter()
            .filter_map(|h| h["label"].as_str().map(|s| s.to_string()))
            .collect();
        assert!(
            labels.iter().filter(|l| *l == "Int").count() >= 2,
            "expected x and y param hints, got {labels:?}"
        );
    }

    #[test]
    fn inlay_field_and_call_projection_hints() {
        let src = r#"
module Demo
type Point {
    val x
    val y
}
val main = {
    val p = Point { x = 1, y = 2 }
    p.x
}
"#;
        let typed = check_source(src, true).expect("typecheck");
        let a = Analysis {
            typed,
            src: src.to_string(),
            files: vec![],
        };
        let hints = hints_for_analysis(&a, None);
        let labels: Vec<_> = hints.iter().filter_map(|h| h["label"].as_str()).collect();
        assert!(
            labels.contains(&"Int"),
            "expected Int hint on field/call, got {labels:?}"
        );
    }
}

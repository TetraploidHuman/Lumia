//! textDocument/inlayHint — binding / param / call-return type hints.

use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_hir::{for_each_expr, Expr, Item};
use lumia_syntax::{byte_to_line_col, line_starts, Span};
use lumia_ty::Type;
use serde_json::{json, Value};

/// LSP InlayHintKind::Type
const KIND_TYPE: i32 = 1;

fn pos_json(src: &str, byte: u32) -> Value {
    let starts = line_starts(src);
    let (line, col) = byte_to_line_col(&starts, lumia_syntax::BytePos(byte));
    json!({
        "line": line.saturating_sub(1),
        "character": col.saturating_sub(1)
    })
}

fn hint_at(src: &str, byte: u32, label: String, padding_left: bool) -> Value {
    json!({
        "position": pos_json(src, byte),
        "label": label,
        "kind": KIND_TYPE,
        "paddingLeft": padding_left,
        "paddingRight": false
    })
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// First whole-word match of `word` in `src[from..to]`; returns byte offset *after* the word.
fn find_word_end(src: &str, word: &str, from: usize, to: usize) -> Option<usize> {
    if word.is_empty() || from >= src.len() {
        return None;
    }
    let to = to.min(src.len());
    if from >= to {
        return None;
    }
    let bytes = src.as_bytes();
    let region = &src[from..to];
    let mut search = 0usize;
    while let Some(rel) = region[search..].find(word) {
        let abs = from + search + rel;
        let before_ok = abs == 0 || !is_ident_byte(bytes[abs - 1]);
        let after = abs + word.len();
        let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok && after_ok {
            return Some(after);
        }
        search += rel + 1;
    }
    None
}

/// Last whole-word match of `word` ending at or before `before`.
fn find_word_end_before(src: &str, word: &str, before: usize) -> Option<usize> {
    let before = before.min(src.len());
    let mut best = None;
    let mut from = 0usize;
    while let Some(end) = find_word_end(src, word, from, before) {
        best = Some(end);
        from = end;
        if from >= before {
            break;
        }
    }
    best
}

/// Parse `{ a, b ->` / `{ ->` / `{ a ->` inside `src[start..end]`.
/// Returns (param_name, byte_after_name) list.
fn lambda_param_ends(src: &str, start: usize, end: usize) -> Vec<(String, usize)> {
    let end = end.min(src.len());
    if start >= end {
        return Vec::new();
    }
    let slice = &src[start..end];
    let Some(brace_rel) = slice.find('{') else {
        return Vec::new();
    };
    let after_brace = start + brace_rel + 1;
    let rest = &src[after_brace..end];
    let Some(arrow_rel) = rest.find("->") else {
        // Block fun `{ … }` with no params.
        return Vec::new();
    };
    let params_src = rest[..arrow_rel].trim();
    if params_src.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cursor = after_brace;
    for part in params_src.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            cursor += part.len() + 1; // + comma
            continue;
        }
        // Skip leading whitespace in this segment relative to params_src positioning:
        // re-find the ident in the original buffer near cursor.
        if let Some(end) = find_word_end(src, trimmed, cursor, end) {
            out.push((trimmed.to_string(), end));
            cursor = end;
        }
    }
    out
}

fn type_of_span<'a>(type_at: &'a [(Span, Type)], span: Span) -> Option<&'a Type> {
    // Prefer the *first* recording: `Let` reuses the value span and would otherwise
    // overwrite the value's type with the let-body type (often Unit).
    type_at
        .iter()
        .find(|(s, _)| s.file == span.file && s.start == span.start && s.end == span.end)
        .map(|(_, t)| t)
}

/// Call result type: prefer the *last* recording at `span`.
/// Callee `Var` used to share the Call span (Fun then Unit); last is the result.
fn type_of_span_last<'a>(type_at: &'a [(Span, Type)], span: Span) -> Option<&'a Type> {
    type_at
        .iter()
        .rev()
        .find(|(s, _)| s.file == span.file && s.start == span.start && s.end == span.end)
        .map(|(_, t)| t)
}

/// Ends of `params` names in order within `src[from..to]` (e.g. `(a, b)` or `{ a, b ->`).
fn param_ends_in_window(
    src: &str,
    params: &[String],
    from: usize,
    to: usize,
) -> Vec<(String, usize)> {
    let mut cursor = from;
    let mut out = Vec::new();
    for p in params {
        if let Some(end) = find_word_end(src, p, cursor, to) {
            out.push((p.clone(), end));
            cursor = end;
        }
    }
    out
}

fn expr_span(e: &Expr) -> Span {
    // Mirror lumia_ty::types::expr_span (not public).
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Bool(_, s)
        | Expr::String(_, s)
        | Expr::Char(_, s)
        | Expr::Unit(s)
        | Expr::Var(_, s)
        | Expr::Break(s)
        | Expr::Continue(s) => *s,
        Expr::Assign { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::Call { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Unary { span, .. }
        | Expr::If { span, .. }
        | Expr::Loop { span, .. }
        | Expr::Seq { span, .. }
        | Expr::BuiltinCall { span, .. }
        | Expr::AdtNew { span, .. }
        | Expr::Return { span, .. }
        | Expr::Alt { span, .. } => *span,
        Expr::Let { value, .. } => expr_span(value),
    }
}

fn collect_vars(ty: &Type, out: &mut Vec<u32>) {
    match ty {
        Type::Var(v) => out.push(*v),
        Type::List(t) | Type::Set(t) => collect_vars(t, out),
        Type::Map(k, v) => {
            collect_vars(k, out);
            collect_vars(v, out);
        }
        Type::Tuple(ts) | Type::TuplePrefix(ts) => {
            for t in ts {
                collect_vars(t, out);
            }
        }
        Type::Adt { params, .. } => {
            for p in params {
                collect_vars(p, out);
            }
        }
        Type::Fun(ps, r, _) => {
            for p in ps {
                collect_vars(p, out);
            }
            collect_vars(r, out);
        }
        Type::Int | Type::Float | Type::Bool | Type::String | Type::Char | Type::Unit => {}
    }
}

/// Num MVP: arithmetic type vars default to Int in IDE hints (DESIGN: numeric default Int).
fn subst_num_vars(ty: &Type, num_vars: &[u32]) -> Type {
    match ty {
        Type::Var(v) if num_vars.contains(v) => Type::Int,
        Type::Var(v) => Type::Var(*v),
        Type::List(t) => Type::List(Box::new(subst_num_vars(t, num_vars))),
        Type::Set(t) => Type::Set(Box::new(subst_num_vars(t, num_vars))),
        Type::Map(k, v) => Type::Map(
            Box::new(subst_num_vars(k, num_vars)),
            Box::new(subst_num_vars(v, num_vars)),
        ),
        Type::Tuple(ts) => Type::Tuple(ts.iter().map(|t| subst_num_vars(t, num_vars)).collect()),
        Type::TuplePrefix(ts) => {
            Type::TuplePrefix(ts.iter().map(|t| subst_num_vars(t, num_vars)).collect())
        }
        Type::Adt { name, params } => Type::Adt {
            name: name.clone(),
            params: params.iter().map(|t| subst_num_vars(t, num_vars)).collect(),
        },
        Type::Fun(ps, r, e) => Type::Fun(
            ps.iter().map(|t| subst_num_vars(t, num_vars)).collect(),
            Box::new(subst_num_vars(r, num_vars)),
            *e,
        ),
        other => other.clone(),
    }
}

fn display_type(ty: &Type, num_vars: &[u32]) -> String {
    let grounded = subst_num_vars(ty, num_vars);
    let names = var_names_for(&grounded);
    pretty_type_with(&grounded, &names)
}

fn var_names_for(ty: &Type) -> rustc_hash::FxHashMap<u32, String> {
    let mut vars = Vec::new();
    collect_vars(ty, &mut vars);
    vars.sort_unstable();
    vars.dedup();
    // Prefer T/U/V… over a/b/c (IDE-facing).
    const LETTERS: &[&str] = &["T", "U", "V", "W", "X", "Y", "Z"];
    vars.iter()
        .enumerate()
        .map(|(i, v)| {
            let name = if i < LETTERS.len() {
                LETTERS[i].to_string()
            } else {
                format!("T{}", i - LETTERS.len() + 1)
            };
            (*v, name)
        })
        .collect()
}

fn pretty_type_with(ty: &Type, names: &rustc_hash::FxHashMap<u32, String>) -> String {
    match ty {
        Type::Var(v) => names.get(v).cloned().unwrap_or_else(|| format!("?{v}")),
        Type::Int => "Int".into(),
        Type::Float => "Float".into(),
        Type::Bool => "Bool".into(),
        Type::String => "String".into(),
        Type::Char => "Char".into(),
        Type::Unit => "Unit".into(),
        Type::List(t) => format!("List[{}]", pretty_type_with(t, names)),
        Type::Set(t) => format!("Set[{}]", pretty_type_with(t, names)),
        Type::Map(k, v) => format!(
            "Map[{}, {}]",
            pretty_type_with(k, names),
            pretty_type_with(v, names)
        ),
        Type::Tuple(ts) => {
            let inner = ts
                .iter()
                .map(|t| pretty_type_with(t, names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner})")
        }
        Type::TuplePrefix(ts) => {
            let inner = ts
                .iter()
                .map(|t| pretty_type_with(t, names))
                .collect::<Vec<_>>()
                .join(", ");
            format!("({inner}, …)")
        }
        Type::Adt { name, params } => {
            if params.is_empty() {
                name.clone()
            } else {
                let inner = params
                    .iter()
                    .map(|t| pretty_type_with(t, names))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}[{inner}]")
            }
        }
        Type::Fun(ps, r, e) => {
            let args = ps
                .iter()
                .map(|t| pretty_type_with(t, names))
                .collect::<Vec<_>>()
                .join(", ");
            let eff = if e.has_io() { " / IO" } else { "" };
            format!("({args}) -> {}{eff}", pretty_type_with(r, names))
        }
    }
}

fn in_range(byte: u32, range: Option<(u32, u32)>) -> bool {
    match range {
        None => true,
        Some((start, end)) => byte >= start && byte <= end,
    }
}

fn range_from_params(src: &str, params: &Value) -> Option<(u32, u32)> {
    let range = params.get("range")?;
    let sl = range["start"]["line"].as_u64()? as u32;
    let sc = range["start"]["character"].as_u64()? as u32;
    let el = range["end"]["line"].as_u64()? as u32;
    let ec = range["end"]["character"].as_u64()? as u32;
    let starts = line_starts(src);
    // Clients (VS Code) often send an end line past EOF; never map that to byte 0
    // via `unwrap_or(0)`, or every hint is filtered out by `in_range`.
    let byte_at = |line: u32, col: u32| -> u32 {
        if starts.is_empty() {
            return 0;
        }
        let last = (starts.len() - 1) as u32;
        if line > last {
            return src.len() as u32;
        }
        let base = starts[line as usize];
        let line_end = if (line as usize + 1) < starts.len() {
            starts[line as usize + 1]
        } else {
            src.len() as u32
        };
        base.saturating_add(col).min(line_end)
    };
    let start = byte_at(sl, sc);
    let end = byte_at(el, ec);
    Some((start.min(end), start.max(end)))
}

fn push_label_hint(
    out: &mut Vec<Value>,
    src: &str,
    byte: u32,
    label: String,
    padding_left: bool,
    range: Option<(u32, u32)>,
) {
    if !in_range(byte, range) {
        return;
    }
    out.push(hint_at(src, byte, label, padding_left));
}

fn push_type_hint(
    out: &mut Vec<Value>,
    src: &str,
    byte: u32,
    ty: &Type,
    padding_left: bool,
    range: Option<(u32, u32)>,
) {
    push_type_hint_nums(out, src, byte, ty, &[], padding_left, range);
}

fn push_type_hint_nums(
    out: &mut Vec<Value>,
    src: &str,
    byte: u32,
    ty: &Type,
    num_vars: &[u32],
    padding_left: bool,
    range: Option<(u32, u32)>,
) {
    push_label_hint(
        out,
        src,
        byte,
        format!(": {}", display_type(ty, num_vars)),
        padding_left,
        range,
    );
}

fn push_fun_param_hints(
    out: &mut Vec<Value>,
    src: &str,
    fun_ty: &Type,
    num_vars: &[u32],
    param_ends: &[(String, usize)],
    range: Option<(u32, u32)>,
) {
    let grounded = subst_num_vars(fun_ty, num_vars);
    let Type::Fun(pts, _, _) = &grounded else {
        return;
    };
    let names = var_names_for(&grounded);
    for (i, (_n, pend)) in param_ends.iter().enumerate() {
        if let Some(pt) = pts.get(i) {
            push_label_hint(
                out,
                src,
                *pend as u32,
                format!(": {}", pretty_type_with(pt, &names)),
                false,
                range,
            );
        }
    }
}

fn emit_arrow_return_hint(
    out: &mut Vec<Value>,
    src: &str,
    search_start: usize,
    search_end: usize,
    fun_ty: &Type,
    num_vars: &[u32],
    range: Option<(u32, u32)>,
) {
    let grounded = subst_num_vars(fun_ty, num_vars);
    let Type::Fun(_, ret, _) = &grounded else {
        return;
    };
    let names = var_names_for(&grounded);
    let Some(slice) = src.get(search_start..search_end) else {
        return;
    };
    let Some(arrow_rel) = slice.find("->") else {
        return;
    };
    let after = search_start + arrow_rel + 2;
    push_label_hint(
        out,
        src,
        after as u32,
        format!(" {}", pretty_type_with(ret, &names)),
        false,
        range,
    );
}

fn collect_expr_hints(
    expr: &Expr,
    src: &str,
    type_at: &[(Span, Type)],
    out: &mut Vec<Value>,
    range: Option<(u32, u32)>,
) {
    for_each_expr(expr, &mut |e| match e {
        Expr::Let { name, value, .. } => {
            let vt = type_of_span(type_at, expr_span(value));
            let Some(ty) = vt else {
                return;
            };
            let before = expr_span(value).start.0 as usize;
            if let Some(end) = find_word_end_before(src, name, before) {
                push_type_hint(out, src, end as u32, ty, false, range);
            }
        }
        Expr::Lambda { span, .. } => {
            let Some(ty) = type_of_span(type_at, *span) else {
                return;
            };
            let start = span.start.0 as usize;
            let end = span.end.0 as usize;
            let found = lambda_param_ends(src, start, end);
            // Nested lambdas lack a stored scheme; treat remaining open vars as-is.
            push_fun_param_hints(out, src, ty, &[], &found, range);
            emit_arrow_return_hint(out, src, start, end, ty, &[], range);
        }
        Expr::Call { span, .. } | Expr::BuiltinCall { span, .. } => {
            let Some(ty) = type_of_span_last(type_at, *span) else {
                return;
            };
            // Unit returns (println, assigns-as-expr) are too noisy at every call site.
            if matches!(ty, Type::Unit) {
                return;
            }
            // Never show a Fun type as a "call result" (stale callee collision).
            if matches!(ty, Type::Fun(..)) {
                return;
            }
            push_type_hint(out, src, span.end.0, ty, true, range);
        }
        _ => {}
    });
}

fn collect_toplevel_hints(a: &Analysis, out: &mut Vec<Value>, range: Option<(u32, u32)>) {
    let src = &a.src;
    let type_at = &a.typed.type_at;
    for item in &a.typed.module.items {
        match item {
            Item::Fun(f) => {
                let body_sp = expr_span(&f.body);
                // Skip items lowered from other files (imports); their names may
                // appear only in `import ….{name}` and would steal binding hints.
                if body_sp.file != 0 {
                    continue;
                }
                let body_start = body_sp.start.0 as usize;
                let scheme = a.typed.fun_schemes.get(&f.name);
                let ty = scheme
                    .map(|s| &s.ty)
                    .or_else(|| a.typed.fun_types.get(&f.name));
                let num_vars: &[u32] = scheme.map(|s| s.num_vars.as_slice()).unwrap_or(&[]);
                if let Some(ty) = ty {
                    // Last `name` before the body — definition, not an earlier call site.
                    if let Some(name_end) = find_word_end_before(src, &f.name, body_start) {
                        push_type_hint_nums(out, src, name_end as u32, ty, num_vars, false, range);
                        if matches!(ty, Type::Fun(..)) {
                            // `val f(a, b) =` and/or `{ a, b ->`
                            let mut found =
                                param_ends_in_window(src, &f.params, name_end, body_start);
                            if found.is_empty() {
                                found = lambda_param_ends(src, name_end, body_start);
                            }
                            push_fun_param_hints(out, src, ty, num_vars, &found, range);
                            emit_arrow_return_hint(
                                out, src, name_end, body_start, ty, num_vars, range,
                            );
                        }
                    }
                }
                collect_expr_hints(&f.body, src, type_at, out, range);
            }
            Item::Val { name, body } => {
                let body_sp = expr_span(body);
                if body_sp.file != 0 {
                    continue;
                }
                let body_start = body_sp.start.0 as usize;
                if let Some(ty) = type_of_span(type_at, body_sp) {
                    if let Some(end) = find_word_end_before(src, name, body_start) {
                        push_type_hint(out, src, end as u32, ty, false, range);
                    }
                }
                collect_expr_hints(body, src, type_at, out, range);
            }
        }
    }
}

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
import std.io.{println}
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
            labels.iter().filter(|l| *l == ": Int").count() >= 2,
            "expected param/local : Int hints, got {labels:?}"
        );
    }

    #[test]
    fn inlay_paren_params_and_call_before_def() {
        // Call site appears *before* `val fun` — binding hint must not stick to the call.
        let src = r#"
module Demo
import std.io.{println}
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
            by_line.iter().filter(|(_, lab)| *lab == ": Int").count() >= 2,
            "expected a/b : Int on val fun(a, b), got {by_line:?}"
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
        let starts = lumia_syntax::line_starts(src);
        let bogus_end_line = starts.len() as u32 + 40;
        let params = serde_json::json!({
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": bogus_end_line, "character": 0 }
            }
        });
        let range = super::range_from_params(src, &params);
        let hints = hints_for_analysis(&a, range);
        assert!(
            !hints.is_empty(),
            "OOB end line must not wipe all inlay hints"
        );
    }
}

//! Collect binding / param / call-return inlay hints from typed analysis.

use super::source::{find_word_end_before, in_range, lambda_param_ends, param_ends_in_window};
use crate::lsp::state::Analysis;
use lumia_hir::{for_each_expr, Expr, Item};
use lumia_syntax::{byte_to_line_col, line_starts, Span};
use lumia_ty::{display_type, expr_span, pretty_type_with, subst_num_vars, var_names_for, Type};
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

fn type_of_span(type_at: &[(Span, Type)], span: Span) -> Option<&Type> {
    // Prefer the *first* recording: `Let` reuses the value span and would otherwise
    // overwrite the value's type with the let-body type (often Unit).
    type_at
        .iter()
        .find(|(s, _)| s.file == span.file && s.start == span.start && s.end == span.end)
        .map(|(_, t)| t)
}

/// Call result type: prefer the *last* recording at `span`.
/// Callee `Var` used to share the Call span (Fun then Unit); last is the result.
fn type_of_span_last(type_at: &[(Span, Type)], span: Span) -> Option<&Type> {
    type_at
        .iter()
        .rev()
        .find(|(s, _)| s.file == span.file && s.start == span.start && s.end == span.end)
        .map(|(_, t)| t)
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

pub(super) fn collect_toplevel_hints(
    a: &Analysis,
    out: &mut Vec<Value>,
    range: Option<(u32, u32)>,
) {
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

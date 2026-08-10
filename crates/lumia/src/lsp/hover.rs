//! textDocument/hover.

use super::cursor::{ident_at, pos_to_byte};
use super::state::{state_lock, Analysis};
use anyhow::Result;
use lumia_syntax::Span;
use lumia_ty::Type;
use serde_json::{json, Value};

pub(super) fn hover_for_analysis(a: &Analysis, line: u32, character: u32) -> Value {
    let byte = pos_to_byte(&a.src, line, character);
    // Prefer tightest spanning type_at entry.
    let mut best: Option<&(Span, Type)> = None;
    for entry in &a.typed.type_at {
        let (sp, _) = entry;
        if sp.file == 0 && sp.start.0 <= byte && byte < sp.end.0.max(sp.start.0 + 1) {
            match best {
                None => best = Some(entry),
                Some((bsp, _)) => {
                    let bw = bsp.end.0.saturating_sub(bsp.start.0);
                    let w = sp.end.0.saturating_sub(sp.start.0);
                    if w < bw {
                        best = Some(entry);
                    }
                }
            }
        }
    }
    if let Some((_, ty)) = best {
        return json!({
            "contents": {
                "kind": "markdown",
                "value": format!("```lumia\n{ty}\n```")
            }
        });
    }
    if let Some(name) = ident_at(&a.src, byte) {
        if let Some(ty) = a.typed.fun_types.get(&name) {
            return json!({
                "contents": {
                    "kind": "markdown",
                    "value": format!("```lumia\n{name}: {ty}\n```")
                }
            });
        }
    }
    Value::Null
}

pub(super) fn on_hover(params: Option<&Value>) -> Result<Value> {
    let Some(params) = params else {
        return Ok(Value::Null);
    };
    let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
    let line = params["position"]["line"].as_u64().unwrap_or(0) as u32;
    let character = params["position"]["character"].as_u64().unwrap_or(0) as u32;
    let st = state_lock();
    let Some(state) = st.as_ref() else {
        return Ok(Value::Null);
    };
    let Some(a) = state.analysis.get(uri) else {
        return Ok(Value::Null);
    };
    Ok(hover_for_analysis(a, line, character))
}

//! Shared LSP completion item helpers.

use rustc_hash::FxHashSet as HashSet;
use serde_json::{json, Value};

pub(super) fn push_item(
    items: &mut Vec<Value>,
    seen: &mut HashSet<String>,
    label: &str,
    kind: u8,
    detail: Option<&str>,
) {
    if !seen.insert(label.to_string()) {
        return;
    }
    let mut v = json!({ "label": label, "kind": kind });
    if let Some(d) = detail {
        v["detail"] = json!(d);
    }
    items.push(v);
}

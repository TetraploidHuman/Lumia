use super::analyze::analyze_buffer;
use super::state::{default_state, state_lock, Analysis};
use lumia_syntax::ColumnMetric;
use rustc_hash::FxHashMap as HashMap;
use serde_json::Value;
use std::sync::Mutex;

static LSP_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) const IMPORTED_ALIAS_SRC: &str = r#"
module Main
import std.io.{println as log}
val main = { log(1) }
"#;

pub(crate) fn with_encoding<R>(enc: ColumnMetric, f: impl FnOnce() -> R) -> R {
    let _lock = LSP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut guard = state_lock();
    let prev = guard.take();
    let mut st = default_state(None);
    st.position_encoding = enc;
    *guard = Some(st);
    drop(guard);
    let out = f();
    *state_lock() = prev;
    out
}

pub(crate) fn analyze_loader(
    uri: &str,
    src: &str,
) -> (Vec<(String, Vec<Value>)>, Option<Analysis>) {
    analyze_buffer(uri, src, &HashMap::default())
}

pub(crate) fn imported_alias_analysis(uri: &str) -> Analysis {
    let (_, analysis) = analyze_loader(uri, IMPORTED_ALIAS_SRC);
    analysis.expect("loader must typecheck untitled std import")
}

pub(crate) fn with_analysis_state<R>(
    uri: &str,
    analysis: Analysis,
    f: impl FnOnce() -> R,
) -> R {
    let prev = state_lock().take();
    let mut st = default_state(None);
    st.analysis.insert(uri.to_string(), analysis);
    *state_lock() = Some(st);
    let out = f();
    *state_lock() = prev;
    out
}

pub(crate) fn with_open_doc_state<R>(uri: &str, src: &str, f: impl FnOnce() -> R) -> R {
    let _lock = LSP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = state_lock().take();
    let mut st = default_state(None);
    st.docs.insert(uri.to_string(), src.to_string());
    *state_lock() = Some(st);
    let out = f();
    *state_lock() = prev;
    out
}

pub(crate) fn with_test_lock<R>(f: impl FnOnce() -> R) -> R {
    let _lock = LSP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

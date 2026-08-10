//! Shared LSP document state and cached analysis.

use crate::load::SourceFile;
use lumia_ty::TypedModule;
use rustc_hash::FxHashMap as HashMap;
use std::sync::Mutex;

pub(super) struct Analysis {
    pub(super) typed: TypedModule,
    /// Primary document source (for hover/completion cursor).
    pub(super) src: String,
    pub(super) files: Vec<SourceFile>,
}

pub(super) struct State {
    pub(super) docs: HashMap<String, String>,
    /// uri → last successful analysis
    pub(super) analysis: HashMap<String, Analysis>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub(super) fn state_lock() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

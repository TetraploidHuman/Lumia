//! Shared LSP document state and cached analysis.

use crate::load::SourceFile;
use lumia_ty::TypedModule;
use rustc_hash::FxHashMap as HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{LazyLock, Mutex};

pub(super) struct Analysis {
    pub(super) typed: TypedModule,
    /// Primary document source (for hover/completion cursor).
    pub(super) src: String,
    pub(super) files: Vec<SourceFile>,
    /// `Span.file` of the open buffer within [`Self::files`] (not always 0 if
    /// the buffer is not the load entry — hover/inlay filter by this id).
    pub(super) buffer_file: u32,
}

pub(super) struct State {
    pub(super) docs: HashMap<String, String>,
    /// uri → last successful analysis
    pub(super) analysis: HashMap<String, Analysis>,
    /// Debounced re-analyze requests (didChange).
    pub(super) analyze_tx: Option<Sender<AnalyzeReq>>,
    /// Mirror of CLI `--parallel` / `--no-parallel` (from `initialize` options).
    pub(super) auto_parallel: bool,
    /// Client advertised `workspace.configuration` (pull settings).
    pub(super) client_supports_configuration: bool,
    /// Next server→client request id.
    pub(super) next_req_id: i64,
    /// Pending `workspace/configuration` request id, if any.
    pub(super) pending_config_req: Option<i64>,
    /// Entry buffer URI → document URIs last touched by `publishDiagnostics`
    /// for that analyze. Re-analyze clears prior import URIs so underlines
    /// do not linger after a successful (or differently failing) check.
    pub(super) last_diag_uris: HashMap<String, Vec<String>>,
}

#[derive(Clone)]
pub(super) struct AnalyzeReq {
    pub(super) uri: String,
    pub(super) text: String,
    pub(super) gen: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static ANALYZE_GEN: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::default()));

pub(super) fn state_lock() -> std::sync::MutexGuard<'static, Option<State>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Current auto-parallel flag (default true when state is unset).
pub(super) fn auto_parallel() -> bool {
    state_lock()
        .as_ref()
        .map(|s| s.auto_parallel)
        .unwrap_or(true)
}

/// Bump per-uri generation and return the new value (latest wins for debounce).
pub(super) fn next_analyze_gen(uri: &str) -> u64 {
    let mut g = ANALYZE_GEN.lock().unwrap_or_else(|e| e.into_inner());
    let e = g.entry(uri.to_string()).or_insert(0);
    *e += 1;
    *e
}

pub(super) fn current_analyze_gen(uri: &str) -> u64 {
    let g = ANALYZE_GEN.lock().unwrap_or_else(|e| e.into_inner());
    g.get(uri).copied().unwrap_or(0)
}

pub(super) fn spawn_analyze_worker() -> Sender<AnalyzeReq> {
    let (tx, rx) = mpsc::channel::<AnalyzeReq>();
    std::thread::Builder::new()
        .name("lumia-lsp-analyze".into())
        .spawn(move || {
            use super::analyze::publish_diagnostics_for;
            use std::time::{Duration, Instant};
            let debounce = Duration::from_millis(120);
            let mut pending: Option<AnalyzeReq> = None;
            let mut deadline = Instant::now();
            loop {
                let timeout = if pending.is_some() {
                    deadline.saturating_duration_since(Instant::now())
                } else {
                    Duration::from_secs(3600)
                };
                match rx.recv_timeout(timeout) {
                    Ok(req) => {
                        pending = Some(req);
                        deadline = Instant::now() + debounce;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(req) = pending.take() {
                            if req.gen == current_analyze_gen(&req.uri) {
                                let _ = publish_diagnostics_for(&req.uri, &req.text);
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn lsp analyze worker");
    tx
}

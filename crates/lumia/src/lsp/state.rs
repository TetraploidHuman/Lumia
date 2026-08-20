//! Shared LSP document state and cached analysis.

use crate::check::PartialProgramCheck;
use crate::load::SourceFile;
use lumia_syntax::ParseOutcome;
use lumia_ty::TypedModule;
use rustc_hash::FxHashMap as HashMap;
use serde_json::Value;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::{cell::Cell, ptr};

pub(super) struct Analysis {
    pub(super) typed: TypedModule,
    /// Primary document source (for hover/completion cursor).
    pub(super) src: String,
    /// Surface syntax for semantic tokens (same `src`; recovering parse).
    pub(super) surface: ParseOutcome,
    pub(super) files: Vec<SourceFile>,
    /// `Span.file` of the open buffer within [`Self::files`] (not always 0 if
    /// the buffer is not the load entry — hover/inlay filter by this id).
    pub(super) buffer_file: u32,
}

impl Analysis {
    pub(super) fn from_typed(
        typed: TypedModule,
        src: String,
        files: Vec<SourceFile>,
        buffer_file: u32,
    ) -> Self {
        let surface = lumia_syntax::parse_module_recovering(&src);
        Self {
            typed,
            src,
            surface,
            files,
            buffer_file,
        }
    }
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
    /// Negotiated LSP position encoding (`utf-8` or `utf-16`).
    pub(super) position_encoding: lumia_syntax::ColumnMetric,
    /// Current workspace folders advertised by the client (multi-root support).
    pub(super) workspace_folders: Vec<PathBuf>,
    /// Set after successful `shutdown` request (LSP lifecycle).
    pub(super) shut_down: bool,
    /// Next server→client request id.
    pub(super) next_req_id: i64,
    /// Pending `workspace/configuration` request id, if any.
    pub(super) pending_config_req: Option<i64>,
    /// Entry buffer URI → document URIs last touched by `publishDiagnostics`
    /// for that analyze. Re-analyze clears prior import URIs so underlines
    /// do not linger after a successful (or differently failing) check.
    pub(super) last_diag_uris: HashMap<String, Vec<String>>,
    /// Per-URI analyze generation (debounce: latest wins). Kept inside [`State`]
    /// so gen and docs share one lock (no parallel `ANALYZE_GEN` mutex).
    pub(super) analyze_gen: HashMap<String, u64>,
    /// Last successful multi-file check for `(ide_entry, overlay fingerprint)`.
    pub(super) program_cache: Option<ProgramCache>,
    /// uri → (source hash, strict format result) — strict parse pretty-print cache.
    /// Caches both success and parse failure for identical source snapshots.
    pub(super) format_cache: HashMap<String, (u64, FormatCacheResult)>,
}

#[derive(Clone)]
pub(super) enum FormatCacheResult {
    Ok(Vec<Value>),
    Err(String),
}

/// Cached load + typecheck for unchanged overlay sets (skip reload on debounce).
pub(super) struct ProgramCache {
    pub ide_entry: PathBuf,
    pub overlay_fp: u64,
    pub auto_parallel: bool,
    pub partial: PartialProgramCheck,
}

/// Stable hash over overlay path + content pairs (sorted by path).
pub(super) fn overlay_fingerprint(overlays: &HashMap<PathBuf, String>) -> u64 {
    let mut pairs: Vec<_> = overlays.iter().collect();
    pairs.sort_by_key(|(p, _)| p.as_os_str());
    let mut h = DefaultHasher::new();
    for (p, s) in pairs {
        p.hash(&mut h);
        s.hash(&mut h);
    }
    h.finish()
}

pub(super) fn source_fingerprint(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

pub(super) fn invalidate_program_cache(state: &mut State) {
    state.program_cache = None;
}

pub(super) fn program_cache_get<'a>(
    state: &'a State,
    ide_entry: &Path,
    overlay_fp: u64,
    auto_parallel: bool,
) -> Option<&'a PartialProgramCheck> {
    state.program_cache.as_ref().and_then(|c| {
        if c.ide_entry == ide_entry && c.overlay_fp == overlay_fp && c.auto_parallel == auto_parallel {
            Some(&c.partial)
        } else {
            None
        }
    })
}

pub(super) fn program_cache_put(
    state: &mut State,
    ide_entry: PathBuf,
    overlay_fp: u64,
    auto_parallel: bool,
    partial: PartialProgramCheck,
) {
    state.program_cache = Some(ProgramCache {
        ide_entry,
        overlay_fp,
        auto_parallel,
        partial,
    });
}

#[derive(Clone)]
pub(super) struct AnalyzeReq {
    pub(super) uri: String,
    pub(super) text: String,
    pub(super) gen: u64,
}

thread_local! {
    // Each LSP "session" (main thread + analyze worker threads) sets its own
    // pointer so multiple LSP servers can coexist in the same process without
    // sharing the same mutable State.
    static SESSION_STATE_PTR: Cell<*const Mutex<Option<State>>> = Cell::new(ptr::null());
}

pub(super) fn create_session_state() -> &'static Mutex<Option<State>> {
    // Leaked per-session: the server runs until process exit. This keeps the
    // returned reference lifetime stable for MutexGuard.
    Box::leak(Box::new(Mutex::new(None)))
}

pub(super) fn set_session_state(state: &'static Mutex<Option<State>>) {
    SESSION_STATE_PTR.with(|c| c.set(state as *const _));
}

fn get_session_state() -> &'static Mutex<Option<State>> {
    SESSION_STATE_PTR.with(|c| {
        let ptr = c.get();
        if ptr.is_null() {
            let st = create_session_state();
            c.set(st as *const _);
            st
        } else {
            unsafe { &*ptr }
        }
    })
}

pub(super) fn state_lock() -> std::sync::MutexGuard<'static, Option<State>> {
    get_session_state().lock().unwrap_or_else(|e| e.into_inner())
}

/// Current auto-parallel flag (default true when state is unset).
pub(super) fn auto_parallel() -> bool {
    state_lock()
        .as_ref()
        .map(|s| s.auto_parallel)
        .unwrap_or(true)
}

/// Negotiated position encoding (LSP default: UTF-16).
pub(super) fn position_encoding() -> lumia_syntax::ColumnMetric {
    state_lock()
        .as_ref()
        .map(|s| s.position_encoding)
        .unwrap_or(lumia_syntax::ColumnMetric::Utf16)
}

/// Bump per-uri generation and return the new value (latest wins for debounce).
pub(super) fn next_analyze_gen(uri: &str) -> u64 {
    let mut st = state_lock();
    let Some(state) = st.as_mut() else {
        return 0;
    };
    let e = state.analyze_gen.entry(uri.to_string()).or_insert(0);
    *e += 1;
    *e
}

pub(super) fn current_analyze_gen(uri: &str) -> u64 {
    state_lock()
        .as_ref()
        .and_then(|s| s.analyze_gen.get(uri).copied())
        .unwrap_or(0)
}

pub(super) fn default_state(analyze_tx: Option<Sender<AnalyzeReq>>) -> State {
    State {
        docs: HashMap::default(),
        analysis: HashMap::default(),
        analyze_tx,
        auto_parallel: true,
        client_supports_configuration: false,
        next_req_id: 1,
        pending_config_req: None,
        last_diag_uris: HashMap::default(),
        analyze_gen: HashMap::default(),
        position_encoding: lumia_syntax::ColumnMetric::Utf16,
        workspace_folders: Vec::new(),
        shut_down: false,
        program_cache: None,
        format_cache: HashMap::default(),
    }
}
pub(super) fn spawn_analyze_worker(
    state: &'static Mutex<Option<State>>,
) -> Sender<AnalyzeReq> {
    let (tx, rx) = mpsc::channel::<AnalyzeReq>();
    std::thread::Builder::new()
        .name("lumia-lsp-analyze".into())
        .spawn(move || {
            set_session_state(state);
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

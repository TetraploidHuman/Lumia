//! Core lowering context (name → local bindings).

use lumia_syntax::{byte_to_line_col, line_starts, Span};
use lumia_ty::{type_at_span, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::rc::Rc;

use crate::ir::Local;

pub(super) struct CoreLowerCtx {
    pub(super) next: u32,
    pub(super) name_to_local: HashMap<String, Local>,
    pub(super) mutables: HashSet<String>,
    pub(super) toplevel_funs: HashSet<String>,
    pub(super) toplevel_vals: HashSet<String>,
    /// Short trait-method names left unresolved until post-mono resolve.
    pub(super) trait_method_names: HashSet<String>,
    /// Top-level functions whose Fun type carries IO (named Call purity).
    pub(super) io_funs: HashSet<lumia_syntax::Sym>,
    /// Zonked expression types from [`lumia_ty::TypedModule::type_at`].
    pub(super) type_at: Rc<[(Span, Type)]>,
    /// `(path_label, source)` per [`Span::file`] for bare `assert(cond)` messages.
    pub(super) assert_files: Rc<[(String, String)]>,
    /// First ICE message (Alt/With residual, failed callee lower, …).
    pub(super) ice: Option<String>,
}

impl CoreLowerCtx {
    pub(super) fn new(
        toplevel_funs: HashSet<String>,
        toplevel_vals: HashSet<String>,
        trait_method_names: HashSet<String>,
        io_funs: HashSet<lumia_syntax::Sym>,
        type_at: Rc<[(Span, Type)]>,
        assert_files: Rc<[(String, String)]>,
    ) -> Self {
        Self {
            next: 0,
            name_to_local: HashMap::default(),
            mutables: HashSet::default(),
            toplevel_funs,
            toplevel_vals,
            trait_method_names,
            io_funs,
            type_at,
            assert_files,
            ice: None,
        }
    }

    pub(super) fn note_ice(&mut self, msg: impl Into<String>) {
        if self.ice.is_none() {
            self.ice = Some(msg.into());
        }
    }

    pub(super) fn fresh(&mut self) -> Local {
        let l = Local(self.next);
        self.next += 1;
        l
    }

    pub(super) fn bind_name(&mut self, name: String, local: Local) {
        self.name_to_local.insert(name, local);
    }

    pub(super) fn bind_mutable(&mut self, name: String, local: Local) {
        self.mutables.insert(name.clone());
        self.bind_name(name, local);
    }

    /// Snapshot of name bindings (not `next` — locals stay unique across scopes).
    pub(super) fn save_bindings(&self) -> (HashMap<String, Local>, HashSet<String>) {
        (self.name_to_local.clone(), self.mutables.clone())
    }

    pub(super) fn restore_bindings(&mut self, saved: (HashMap<String, Local>, HashSet<String>)) {
        self.name_to_local = saved.0;
        self.mutables = saved.1;
    }

    pub(super) fn type_of_span(&self, span: Span) -> Option<Type> {
        type_at_span(&self.type_at, span)
    }

    /// Default failure text for bare `assert(cond)` (`path:line: assert failed`).
    pub(super) fn assert_fail_message(&self, span: Span) -> Option<String> {
        if self.assert_files.is_empty() {
            return None;
        }
        let (path, src) = self
            .assert_files
            .get(span.file as usize)
            .or_else(|| self.assert_files.first())?;
        let starts = line_starts(src);
        let (line, _) = byte_to_line_col(&starts, span.start);
        Some(format!("{path}:{line}: assert failed"))
    }
}

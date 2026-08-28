//! Lowering context and errors.

use crate::ast::CtorInfo;
use lumi_syntax::{LocatedError, Span};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;

/// Lowering / exhaustiveness failure (shared [`LocatedError`] shape).
pub type LowerError = LocatedError;

/// Options for syntax → HIR lowering (Phase C capabilities).
#[derive(Debug, Clone)]
pub struct LowerOptions {
    /// Fuse `map`/`filter`/`fold` pipelines (deforestation). Default on.
    pub hof_fuse: bool,
}

impl Default for LowerOptions {
    fn default() -> Self {
        Self { hof_fuse: true }
    }
}

/// Explicit lowering context (ctors, products, parallel-safety sets).
pub struct LowerCtx {
    pub(crate) ctors: HashMap<String, CtorInfo>,
    pub(crate) product_fields: HashMap<String, (String, usize)>,
    #[allow(dead_code)] // retained for ambiguous-field diagnostics / future lower
    pub(crate) product_field_owners: HashMap<String, Vec<(String, usize)>>,
    pub(crate) ambiguous_product_fields: HashSet<String>,
    pub(crate) products: HashMap<String, Vec<String>>,
    err: RefCell<Option<LowerError>>,
    pub(crate) toplevel_funs: HashSet<String>,
    pub(crate) toplevel_fold_assoc: HashSet<String>,
    pub(crate) hof_fuse: bool,
}

impl LowerCtx {
    /// Empty context for post-lower desugars (e.g. demoting `ListParMap` in ty).
    /// Loop skeletons do not consult ctor/product tables.
    pub fn empty() -> Self {
        Self {
            ctors: HashMap::default(),
            products: HashMap::default(),
            product_fields: HashMap::default(),
            product_field_owners: HashMap::default(),
            ambiguous_product_fields: HashSet::default(),
            err: RefCell::new(None),
            toplevel_funs: HashSet::default(),
            toplevel_fold_assoc: HashSet::default(),
            hof_fuse: true,
        }
    }

    pub(crate) fn new(
        ctors: HashMap<String, CtorInfo>,
        products: HashMap<String, Vec<String>>,
        product_fields: HashMap<String, (String, usize)>,
        product_field_owners: HashMap<String, Vec<(String, usize)>>,
        ambiguous_product_fields: HashSet<String>,
        toplevel_funs: HashSet<String>,
        toplevel_fold_assoc: HashSet<String>,
        opts: &LowerOptions,
    ) -> Self {
        Self {
            ctors,
            products,
            product_fields,
            product_field_owners,
            ambiguous_product_fields,
            err: RefCell::new(None),
            toplevel_funs,
            toplevel_fold_assoc,
            hof_fuse: opts.hof_fuse,
        }
    }

    pub(crate) fn set_err(&self, msg: String, span: Span) {
        let mut slot = self.err.borrow_mut();
        if slot.is_none() {
            *slot = Some(LowerError { message: msg, span });
        }
    }

    pub(crate) fn take_err(&self) -> Option<LowerError> {
        self.err.borrow_mut().take()
    }

    pub(crate) fn lookup_ctor(&self, name: &str) -> Option<CtorInfo> {
        self.ctors.get(name).cloned()
    }

    pub(crate) fn lookup_product_field(&self, name: &str) -> Option<(String, usize)> {
        self.product_fields.get(name).cloned()
    }

    pub(crate) fn is_ambiguous_product_field(&self, name: &str) -> bool {
        self.ambiguous_product_fields.contains(name)
    }

    pub(crate) fn lookup_product(&self, name: &str) -> Option<Vec<String>> {
        self.products.get(name).cloned()
    }

    pub(crate) fn is_toplevel_fun(&self, name: &str) -> bool {
        self.toplevel_funs.contains(name)
    }

    pub(crate) fn is_toplevel_fold_assoc(&self, name: &str) -> bool {
        self.toplevel_fold_assoc.contains(name)
    }
}

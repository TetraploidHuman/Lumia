//! Lowering context and errors.

use crate::ast::CtorInfo;
use lumia_syntax::{LocatedError, Span};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;

/// Lowering / exhaustiveness failure (shared [`LocatedError`] shape).
pub type LowerError = LocatedError;

/// Explicit lowering context (ctors, products, parallel-safety sets).
pub struct LowerCtx {
    pub(crate) ctors: HashMap<String, CtorInfo>,
    pub(crate) product_fields: HashMap<String, (String, usize)>,
    pub(crate) ambiguous_product_fields: HashSet<String>,
    pub(crate) products: HashMap<String, Vec<String>>,
    /// Accumulated lower diagnostics (block lower is inside-out; must not keep only the last).
    errs: RefCell<Vec<LowerError>>,
    pub(crate) toplevel_funs: HashSet<String>,
    pub(crate) toplevel_fold_assoc: HashSet<String>,
}

impl LowerCtx {
    pub(crate) fn new(
        ctors: HashMap<String, CtorInfo>,
        products: HashMap<String, Vec<String>>,
        product_fields: HashMap<String, (String, usize)>,
        ambiguous_product_fields: HashSet<String>,
        toplevel_funs: HashSet<String>,
        toplevel_fold_assoc: HashSet<String>,
    ) -> Self {
        Self {
            ctors,
            products,
            product_fields,
            ambiguous_product_fields,
            errs: RefCell::new(Vec::new()),
            toplevel_funs,
            toplevel_fold_assoc,
        }
    }

    pub(crate) fn set_err(&self, msg: String, span: Span) {
        self.errs
            .borrow_mut()
            .push(LowerError { message: msg, span });
    }

    pub(crate) fn take_errs(&self) -> Vec<LowerError> {
        let mut errs = std::mem::take(&mut *self.errs.borrow_mut());
        errs.sort_by(|a, b| {
            (a.span.file, a.span.start.0, a.span.end.0).cmp(&(
                b.span.file,
                b.span.start.0,
                b.span.end.0,
            ))
        });
        errs
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

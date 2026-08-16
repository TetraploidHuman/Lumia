//! Lambda lifting and capture analysis.

mod captures;
mod channel_hint;
pub(crate) mod float_abi;
mod float_cap_fixup;
mod heap;
mod rewrite;

pub(crate) use channel_hint::refine_channel_elem_hint;
pub(crate) use float_cap_fixup::fixup_closure_float_caps;
pub(crate) use rewrite::lift_lambdas;

use crate::ir::CoreModule;
use lumia_ty::{Effect, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::cell::RefCell;

thread_local! {
    /// FunKind-lifted names for table-only [`fun_ty_from_tables`] (no CoreFun).
    static LIFTED_LAMBDA_NAMES: RefCell<HashSet<String>> = RefCell::new(HashSet::default());
}

/// Run `f` with FunKind-derived lifted names visible to float_abi / channel_hint
/// table lookups (replaces `__lam_*` key recovery).
pub(crate) fn with_lifted_lambda_names<R>(lifted: HashSet<String>, f: impl FnOnce() -> R) -> R {
    LIFTED_LAMBDA_NAMES.with(|cell| {
        let prev = std::mem::replace(&mut *cell.borrow_mut(), lifted);
        let out = f();
        *cell.borrow_mut() = prev;
        out
    })
}

/// Record a newly lifted lambda while [`with_lifted_lambda_names`] is active.
pub(crate) fn note_lifted_lambda_name(name: String) {
    LIFTED_LAMBDA_NAMES.with(|cell| {
        cell.borrow_mut().insert(name);
    });
}

fn current_lifted_lambda_names<R>(f: impl FnOnce(&HashSet<String>) -> R) -> R {
    LIFTED_LAMBDA_NAMES.with(|cell| f(&cell.borrow()))
}

/// Names of lifted lambdas ([`crate::CoreFun::is_lifted_lambda`]).
pub(crate) fn lifted_lambda_names(module: &CoreModule) -> HashSet<String> {
    module
        .functions
        .iter()
        .filter(|f| f.is_lifted_lambda())
        .map(|f| f.name.clone())
        .collect()
}

/// Build a user-facing `Fun` type from tables, dropping a leading env Int for
/// lifted closures ([`crate::FunKind::LiftedLambda`]).
///
/// `lifted` must come from [`lifted_lambda_names`] (FunKind), not name prefixes.
pub(crate) fn fun_ty_from_tables(
    name: &str,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
    lifted: &HashSet<String>,
) -> Option<Type> {
    let ret = fun_ret_tys.get(name)?.clone();
    let mut params = fun_param_tys.get(name).cloned().unwrap_or_default();
    let is_lifted = lifted.contains(name);
    if is_lifted
        && params
            .first()
            .is_some_and(|p| matches!(p, Type::Int | Type::Var(_)))
        && params.len() > 1
    {
        params.remove(0);
    }
    Some(Type::Fun(params, Box::new(ret), Effect::pure()))
}

/// Like [`fun_ty_from_tables`], using the set installed by [`with_lifted_lambda_names`].
pub(crate) fn fun_ty_from_tables_tls(
    name: &str,
    fun_ret_tys: &HashMap<String, Type>,
    fun_param_tys: &HashMap<String, Vec<Type>>,
) -> Option<Type> {
    current_lifted_lambda_names(|lifted| {
        fun_ty_from_tables(name, fun_ret_tys, fun_param_tys, lifted)
    })
}

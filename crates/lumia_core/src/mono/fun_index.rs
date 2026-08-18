//! Fast name → function lookup for monomorphization.
//!
//! [`FunIndex`] borrows a function table. Call sites that must mutate bodies
//! while indexing build a **signature shadow** ([`signature_shadow`]) — empty
//! bodies, same ABI fields — so the live `CoreFun::body` stays in place (no
//! `mem::replace` empty-Block dance).

use crate::ir::{Block, CoreFun, CoreModule, FunId};
use lumia_ty::Type;
use rustc_hash::FxHashMap as HashMap;

use super::key::MonoKey;

/// Immutable name index into a function table (rebuilt when the table grows).
pub(crate) struct FunIndex<'a> {
    funs: &'a [CoreFun],
    by_name: HashMap<&'a str, usize>,
    pub(crate) sum_max_arity: &'a HashMap<String, usize>,
    pub(crate) trait_methods: &'a HashMap<(String, String), Vec<String>>,
    pub(crate) channel_elem_hint: Option<&'a Type>,
}

impl<'a> FunIndex<'a> {
    pub(crate) fn new(
        funs: &'a [CoreFun],
        sum_max_arity: &'a HashMap<String, usize>,
        trait_methods: &'a HashMap<(String, String), Vec<String>>,
        channel_elem_hint: Option<&'a Type>,
    ) -> Self {
        let mut by_name = HashMap::with_capacity_and_hasher(funs.len(), Default::default());
        for (i, f) in funs.iter().enumerate() {
            by_name.insert(f.name.as_str(), i);
        }
        Self {
            funs,
            by_name,
            sum_max_arity,
            trait_methods,
            channel_elem_hint,
        }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&'a CoreFun> {
        self.by_name.get(name).map(|&i| &self.funs[i])
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub(crate) fn funs(&self) -> &'a [CoreFun] {
        self.funs
    }

    /// Module-local [`FunId`] for `name`, if present.
    pub(crate) fn id_of(&self, name: &str) -> Option<FunId> {
        self.by_name.get(name).copied().map(|i| FunId(i as u32))
    }

    /// Stamp FunRef ids on a mono key (O(1) per FunRef).
    pub(crate) fn stamp_funref_ids(&self, key: &mut MonoKey) {
        for k in &mut key.0 {
            if let super::key::MonoKind::FunRef(fr) = k {
                if fr.id.is_none() {
                    fr.id = self.id_of(&fr.name);
                }
            }
        }
    }
}

/// Clone ABI fields with an empty body — safe FunIndex while mutating live bodies.
pub(crate) fn signature_shadow(funs: &[CoreFun]) -> Vec<CoreFun> {
    let empty = Block {
        ops: Vec::new(),
        result: None,
    };
    funs.iter()
        .map(|f| CoreFun {
            name: f.name.clone(),
            params: f.params.clone(),
            param_names: f.param_names.clone(),
            param_tys: f.param_tys.clone(),
            body: empty.clone(),
            ret_ty: f.ret_ty.clone(),
            effect: f.effect,
            is_main: f.is_main,
            memo: None,
            external: f.external.clone(),
            foreign_abi: f.foreign_abi,
            escaping: Default::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: f.scheme_poly,
            mono_of: f.mono_of.clone(),
            kind: f.kind,
        })
        .collect()
}

/// Shadow + owned module tables for [`FunIndex`] across body mutation.
pub(crate) struct SigShadow {
    pub funs: Vec<CoreFun>,
    pub sum_max_arity: HashMap<String, usize>,
    pub trait_methods: HashMap<(String, String), Vec<String>>,
    pub channel_elem_hint: Option<Type>,
}

impl SigShadow {
    pub(crate) fn from_module(module: &CoreModule) -> Self {
        Self {
            funs: signature_shadow(&module.functions),
            sum_max_arity: module.sum_max_arity.clone(),
            trait_methods: module.trait_methods.clone(),
            channel_elem_hint: module.channel_elem_hint.clone(),
        }
    }

    pub(crate) fn index(&self) -> FunIndex<'_> {
        FunIndex::new(
            &self.funs,
            &self.sum_max_arity,
            &self.trait_methods,
            self.channel_elem_hint.as_ref(),
        )
    }
}

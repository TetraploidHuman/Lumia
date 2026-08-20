//! Owned module-wide ABI lookup tables shared by mid-end passes and codegen seed.
//!
//! Replaces hand-rolled `fun_ret_tys` / `fun_param_tys` HashMap clones at each
//! pass entry (float_cap_fixup, channel_hint, …) and the Core blackboard clone
//! into codegen [`FunTables`](lumia_codegen side). Borrowed views feed
//! [`crate::CodegenTypeTables`] / [`crate::InferValueCtx`].

use crate::ir::{CoreFun, CoreModule, Local, Op, Value};
use lumia_syntax::Sym;
use lumia_ty::Type;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Snapshot of per-function ABI types plus Core analysis side tables.
#[derive(Clone, Debug, Default)]
pub struct ModuleTables {
    pub fun_ret_tys: HashMap<Sym, Type>,
    pub fun_param_tys: HashMap<Sym, Vec<Type>>,
    /// Functions whose body is a pure alias of param0 (identity forwarders).
    pub fun_param0_identity: HashSet<Sym>,
    /// Trait short-name → mangled impls (mono UFCS / stubs).
    pub trait_methods: HashMap<(Sym, Sym), Vec<Sym>>,
    pub hash_adts: HashSet<Sym>,
    pub adt_variant_names: HashMap<Sym, Vec<String>>,
    pub sum_max_arity: HashMap<Sym, usize>,
    pub channel_elem_hint: Option<Type>,
    pub channel_elem_by_local: HashMap<u32, Type>,
}

/// `f(x, …) = x` via Local-only forwarding (no Assign / non-Local Lets).
pub fn core_fun_is_param0_identity(f: &CoreFun) -> bool {
    let Some(p0) = f.params.first().map(|p| p.0) else {
        return false;
    };
    let Some(Local(result)) = f.body.result else {
        return false;
    };
    let mut root: HashMap<u32, u32> = HashMap::default();
    root.insert(p0, p0);
    for op in &f.body.ops {
        match op {
            Op::Let {
                local,
                value: Value::Local(Local(src)),
                ..
            } => {
                if let Some(&r) = root.get(src) {
                    root.insert(local.0, r);
                } else {
                    return false;
                }
            }
            Op::Let { .. } | Op::Assign { .. } => return false,
            _ => {}
        }
    }
    root.get(&result) == Some(&p0)
}

impl ModuleTables {
    pub fn from_module(module: &CoreModule) -> Self {
        let mut fun_ret_tys = HashMap::default();
        let mut fun_param_tys = HashMap::default();
        let mut fun_param0_identity = HashSet::default();
        fun_ret_tys.reserve(module.functions.len());
        fun_param_tys.reserve(module.functions.len());
        for f in &module.functions {
            fun_ret_tys.insert(f.name.clone(), f.ret_ty.clone());
            fun_param_tys.insert(f.name.clone(), f.param_tys.clone());
            if core_fun_is_param0_identity(f) {
                fun_param0_identity.insert(f.name.clone());
            }
        }
        Self {
            fun_ret_tys,
            fun_param_tys,
            fun_param0_identity,
            trait_methods: module.trait_methods.clone(),
            hash_adts: module.hash_adts.clone(),
            adt_variant_names: module.adt_variant_names.clone(),
            sum_max_arity: module.sum_max_arity.clone(),
            channel_elem_hint: module.channel_elem_hint.clone(),
            channel_elem_by_local: module.channel_elem_by_local.clone(),
        }
    }

    /// Consume into owned maps for passes that insert lifted/`$` clones in place.
    pub fn into_maps(self) -> (HashMap<Sym, Type>, HashMap<Sym, Vec<Type>>) {
        (self.fun_ret_tys, self.fun_param_tys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, FunKind};
    use lumia_ty::Effect;

    #[test]
    fn from_module_indexes_ret_and_params() {
        let f = CoreFun {
            name: "add".into(),
            params: vec![Local(0), Local(1)],
            param_names: vec!["a".into(), "b".into()],
            param_tys: vec![Type::Int, Type::Int],
            body: Block {
                ops: vec![],
                result: Some(Local(0)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        };
        let id = CoreFun {
            name: "id".into(),
            params: vec![Local(0)],
            param_names: vec!["x".into()],
            param_tys: vec![Type::Int],
            body: Block {
                ops: vec![],
                result: Some(Local(0)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        };
        let mut m = CoreModule::with_functions("M", vec![f, id]);
        m.hash_adts.insert(Sym::from("Point"));
        m.sum_max_arity.insert(Sym::from("Option"), 1);
        m.channel_elem_hint = Some(Type::Float);
        m.channel_elem_by_local.insert(3, Type::Int);
        m.trait_methods.insert(
            (Sym::from("Show"), Sym::from("show")),
            vec![Sym::from("Show$Int")],
        );
        let t = ModuleTables::from_module(&m);
        assert_eq!(t.fun_ret_tys.get("add"), Some(&Type::Int));
        assert_eq!(
            t.fun_param_tys.get("add"),
            Some(&vec![Type::Int, Type::Int])
        );
        assert!(t.hash_adts.contains("Point"));
        assert_eq!(t.sum_max_arity.get("Option"), Some(&1));
        assert_eq!(t.channel_elem_hint, Some(Type::Float));
        assert_eq!(t.channel_elem_by_local.get(&3), Some(&Type::Int));
        assert_eq!(
            t.trait_methods
                .get(&(Sym::from("Show"), Sym::from("show"))),
            Some(&vec![Sym::from("Show$Int")])
        );
        assert!(t.fun_param0_identity.contains("id"));
        assert!(t.fun_param0_identity.contains("add")); // result = Local(0) = param0
        assert!(!core_fun_is_param0_identity(&CoreFun {
            name: "bad".into(),
            params: vec![Local(0)],
            param_names: vec!["x".into()],
            param_tys: vec![Type::Int],
            body: Block {
                ops: vec![Op::Let {
                    local: Local(1),
                    value: Value::Int(1),
                    pure_region: true,
                }],
                result: Some(Local(1)),
            },
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: HashSet::default(),
            nsw_binop_locals: Default::default(),
            safe_divisor_locals: Default::default(),
            nonneg_iv_load_locals: Default::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        }));
    }
}

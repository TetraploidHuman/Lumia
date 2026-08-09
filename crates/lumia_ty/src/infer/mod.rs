//! Hindley–Milner inference engine.

mod builtins;
mod calls;
mod expr;
mod module;
mod unify;

pub use module::{
    infer_module, infer_module_with_options, infer_module_with_visibility, InferOptions,
};

use crate::types::{at, Effect, NameVisibility, Scheme, Type, TypeError};
use std::collections::{HashMap, HashSet};

pub(crate) struct Infer {
    pub(crate) next_var: u32,
    pub(crate) next_eff: u32,
    pub(crate) subst: HashMap<u32, Type>,
    pub(crate) eff_subst: HashMap<u32, Effect>,
    pub(crate) env: Vec<HashMap<String, Scheme>>,
    /// Parallel to `env`: names bound with `var` (assignable) in that scope.
    pub(crate) mutables: Vec<HashSet<String>>,
    pub(crate) type_at: Vec<(lumia_syntax::Span, Type)>,
    pub(crate) decls: HashMap<String, lumia_syntax::Span>,
    pub(crate) vis: NameVisibility,
    /// File id of the function/val body currently being inferred.
    pub(crate) current_file: u32,
    /// Type names with `instance Ord for T` (MVP type-class wiring).
    pub(crate) ord_instances: HashSet<String>,
    /// Type names with `instance Num for T` (`+`/`*` method dispatch).
    pub(crate) num_instances: HashSet<String>,
    /// Type vars used in arithmetic — may only resolve to Int/Float (Num MVP).
    pub(crate) num_vars: HashSet<u32>,
    /// Type var → required `(trait, method)` from deferred poly UFCS.
    pub(crate) trait_vars: HashMap<u32, Vec<(String, String)>>,
    /// All `(trait, type)` instances (incl. auto-derived) for constraint checks.
    pub(crate) instances: HashSet<(String, String)>,
    /// `(type, method)` → mangled instance methods (from HIR).
    pub(crate) trait_methods: HashMap<(String, String), Vec<String>>,
    /// method → unique trait name (error if the same method appears on multiple traits).
    pub(crate) method_trait: HashMap<String, String>,
    /// UFCS calls rewritten to mangled `__Trait_Type_method` (span of Call expr).
    pub(crate) ufcs_rewrites: HashMap<lumia_syntax::Span, String>,
    /// Product type name → field names (from HIR module tables).
    pub(crate) products: HashMap<String, Vec<String>>,
}

impl Infer {
    pub(crate) fn new(vis: NameVisibility) -> Self {
        let mut builtins = HashMap::new();
        // println: Int or String → Unit / IO (overloads via Call special-case)
        builtins.insert(
            "println".into(),
            Scheme::mono(Type::Fun(
                vec![Type::Int],
                Box::new(Type::Unit),
                Effect::io(),
            )),
        );
        // listOf / mapOf / setOf: 0-arg empty; Call site special-cases arity
        builtins.insert(
            "listOf".into(),
            Scheme::mono(Type::Fun(
                vec![],
                Box::new(Type::List(Box::new(Type::Int))),
                Effect::pure(),
            )),
        );
        builtins.insert(
            "mapOf".into(),
            Scheme::mono(Type::Fun(
                vec![],
                Box::new(Type::Map(Box::new(Type::Int), Box::new(Type::Int))),
                Effect::pure(),
            )),
        );
        builtins.insert(
            "setOf".into(),
            Scheme::mono(Type::Fun(
                vec![],
                Box::new(Type::Set(Box::new(Type::Int))),
                Effect::pure(),
            )),
        );
        // Bind std import aliases (`println as log`) to the same schemes.
        for (alias, canon) in &vis.builtin_aliases {
            if let Some(scheme) = builtins.get(canon).cloned() {
                builtins.insert(alias.clone(), scheme);
            }
        }
        Self {
            next_var: 0,
            next_eff: 0,
            subst: HashMap::new(),
            eff_subst: HashMap::new(),
            env: vec![builtins],
            mutables: vec![HashSet::new()],
            type_at: Vec::new(),
            decls: HashMap::new(),
            vis,
            current_file: 0,
            ord_instances: HashSet::new(),
            num_instances: HashSet::new(),
            num_vars: HashSet::new(),
            trait_vars: HashMap::new(),
            instances: HashSet::new(),
            trait_methods: HashMap::new(),
            method_trait: HashMap::new(),
            ufcs_rewrites: HashMap::new(),
            products: HashMap::new(),
        }
    }

    pub(crate) fn check_name_visible(
        &self,
        name: &str,
        span: lumia_syntax::Span,
    ) -> Result<(), TypeError> {
        if self.vis.allows(name, self.current_file) {
            Ok(())
        } else {
            Err(at(
                span,
                format!("`{name}` is private or not imported into this module"),
            ))
        }
    }

    pub(crate) fn fresh(&mut self) -> Type {
        let v = self.next_var;
        self.next_var += 1;
        Type::Var(v)
    }

    pub(crate) fn fresh_eff(&mut self) -> Effect {
        let v = self.next_eff;
        self.next_eff += 1;
        Effect::Var(v)
    }

    pub(crate) fn push(&mut self) {
        self.env.push(HashMap::new());
        self.mutables.push(HashSet::new());
    }

    pub(crate) fn pop(&mut self) {
        self.env.pop();
        self.mutables.pop();
    }

    pub(crate) fn bind(&mut self, name: String, ty: Type) {
        self.bind_scheme(name, Scheme::mono(ty), false);
    }

    pub(crate) fn bind_mut(&mut self, name: String, ty: Type, mutable: bool) {
        self.bind_scheme(name, Scheme::mono(ty), mutable);
    }

    pub(crate) fn bind_scheme(&mut self, name: String, scheme: Scheme, mutable: bool) {
        self.env.last_mut().unwrap().insert(name.clone(), scheme);
        let m = self.mutables.last_mut().unwrap();
        if mutable {
            m.insert(name);
        } else {
            m.remove(&name);
        }
    }

    pub(crate) fn lookup(&mut self, name: &str) -> Option<Type> {
        let scheme = self
            .env
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())?;
        Some(self.instantiate(&scheme))
    }

    /// True when the binding that `lookup` would see was introduced with `var`.
    pub(crate) fn is_mutable(&self, name: &str) -> bool {
        for (scope, muts) in self.env.iter().zip(self.mutables.iter()).rev() {
            if scope.contains_key(name) {
                return muts.contains(name);
            }
        }
        false
    }
}

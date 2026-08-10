//! Hindley–Milner inference engine.

mod builtins;
mod calls;
mod expr;
mod module;
mod state;
mod unify;

pub use module::{
    infer_module, infer_module_recovering, infer_module_with_options, infer_module_with_visibility,
    InferOptions,
};

use crate::types::{at, Effect, NameVisibility, Scheme, Type, TypeError};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use state::{AltReturnState, EnvState, ProductState, SubstState, TraitState};

pub(crate) struct Infer {
    pub(crate) uni: SubstState,
    pub(crate) scopes: EnvState,
    pub(crate) traits: TraitState,
    pub(crate) products: ProductState,
    pub(crate) ctrl: AltReturnState,
    pub(crate) type_at: Vec<(lumia_syntax::Span, Type)>,
    pub(crate) decls: HashMap<String, lumia_syntax::Span>,
    pub(crate) vis: NameVisibility,
    /// File id of the function/val body currently being inferred.
    pub(crate) current_file: u32,
}

impl Infer {
    pub(crate) fn new(vis: NameVisibility) -> Self {
        let mut builtins = HashMap::default();
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
        Self {
            uni: SubstState::default(),
            scopes: EnvState {
                env: vec![builtins],
                mutables: vec![HashSet::default()],
            },
            traits: TraitState::default(),
            products: ProductState::default(),
            ctrl: AltReturnState::default(),
            type_at: Vec::new(),
            decls: HashMap::default(),
            vis,
            current_file: 0,
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
        let v = self.uni.next_var;
        self.uni.next_var += 1;
        Type::Var(v)
    }

    pub(crate) fn fresh_eff(&mut self) -> Effect {
        let v = self.uni.next_eff;
        self.uni.next_eff += 1;
        Effect::Var(v)
    }

    pub(crate) fn push(&mut self) {
        self.scopes.env.push(HashMap::default());
        self.scopes.mutables.push(HashSet::default());
    }

    pub(crate) fn pop(&mut self) {
        self.scopes.env.pop();
        self.scopes.mutables.pop();
    }

    pub(crate) fn bind(&mut self, name: String, ty: Type) {
        self.bind_scheme(name, Scheme::mono(ty), false);
    }

    pub(crate) fn bind_mut(&mut self, name: String, ty: Type, mutable: bool) {
        self.bind_scheme(name, Scheme::mono(ty), mutable);
    }

    pub(crate) fn bind_scheme(&mut self, name: String, scheme: Scheme, mutable: bool) {
        self.scopes
            .env
            .last_mut()
            .unwrap()
            .insert(name.clone(), scheme);
        let m = self.scopes.mutables.last_mut().unwrap();
        if mutable {
            m.insert(name);
        } else {
            m.remove(&name);
        }
    }

    pub(crate) fn lookup(&mut self, name: &str) -> Option<Type> {
        let scheme = self
            .scopes
            .env
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())?;
        Some(self.instantiate(&scheme))
    }

    /// True when the binding that `lookup` would see was introduced with `var`.
    pub(crate) fn is_mutable(&self, name: &str) -> bool {
        for (scope, muts) in self
            .scopes
            .env
            .iter()
            .zip(self.scopes.mutables.iter())
            .rev()
        {
            if scope.contains_key(name) {
                return muts.contains(name);
            }
        }
        false
    }
}

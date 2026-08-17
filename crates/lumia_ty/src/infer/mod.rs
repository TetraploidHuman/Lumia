//! Hindley–Milner inference engine.

mod builtins;
mod binding_order;
mod calls;
mod expr;
mod free_vars;
mod module;
mod prelude_ctors;
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
        // Do **not** seed `println` / `assert` / `readStdin` here: free *calls*
        // lower to `BuiltinCall` (typed via `infer_builtin_call`); first-class use
        // requires `import std.io.{…}` (see std/io.lm). Seeding `println` made
        // `val f = println` check-ok on a lone library file while codegen failed
        // (`unbound mutable`) and multi-file package check correctly rejected it.
        // Collection ctors: [`lumia_hir::PRELUDE_CTORS`]; call sites specialize in
        // `prelude_ctors`. First-class / alias use needs ∀ schemes (not Int stubs).
        // Quantified ids must stay below `next_var` so they never collide with `fresh()`.
        let mut next_var = 0u32;
        let poly = |vars: Vec<u32>, ty: Type| -> Scheme {
            let mut sch = Scheme::mono(ty);
            sch.vars = vars;
            sch
        };
        for sn in lumia_hir::PRELUDE_CTORS {
            let sch = match sn.name {
                "listOf" => {
                    let a = next_var;
                    next_var += 1;
                    poly(
                        vec![a],
                        Type::Fun(
                            vec![],
                            Box::new(Type::List(Box::new(Type::Var(a)))),
                            Effect::pure(),
                        ),
                    )
                }
                "mapOf" => {
                    let k = next_var;
                    next_var += 1;
                    let v = next_var;
                    next_var += 1;
                    poly(
                        vec![k, v],
                        Type::Fun(
                            vec![],
                            Box::new(Type::Map(Box::new(Type::Var(k)), Box::new(Type::Var(v)))),
                            Effect::pure(),
                        ),
                    )
                }
                "setOf" => {
                    let a = next_var;
                    next_var += 1;
                    poly(
                        vec![a],
                        Type::Fun(
                            vec![],
                            Box::new(Type::Set(Box::new(Type::Var(a)))),
                            Effect::pure(),
                        ),
                    )
                }
                other => panic!("lumia: unhandled PRELUDE_CTOR `{other}`"),
            };
            builtins.insert(sn.name.into(), sch);
        }
        Self {
            uni: SubstState {
                next_var,
                ..SubstState::default()
            },
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

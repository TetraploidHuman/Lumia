//! Grouped state for [`super::Infer`].

use crate::alt::AltKind;
use crate::types::{Effect, Scheme, Type};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

/// Unification / substitution / Num var tracking.
#[derive(Default)]
pub(crate) struct SubstState {
    pub(crate) next_var: u32,
    pub(crate) next_eff: u32,
    pub(crate) subst: HashMap<u32, Type>,
    pub(crate) eff_subst: HashMap<u32, Effect>,
    /// Type vars used in arithmetic — may only resolve to Int/Float (Num MVP).
    pub(crate) num_vars: HashSet<u32>,
    /// Type vars used in `<`/`<=`/`>`/`>=` — may only resolve to Ord scalars/ADTs.
    pub(crate) ord_vars: HashSet<u32>,
    /// Type vars used in `==`/`!=` — must not resolve to Fun (DESIGN: no ref eq).
    pub(crate) eq_vars: HashSet<u32>,
    /// Type vars used with `.len()` — List/Set/Map/String only (not default List).
    pub(crate) len_vars: HashSet<u32>,
    /// Type vars used with `.concat` when both sides open — List or String.
    pub(crate) concat_vars: HashSet<u32>,
    /// Type vars used with `.contains` — Map/Set/String only (not List).
    pub(crate) contains_vars: HashSet<u32>,
    /// Type vars used with open `.set` — List or Map.
    pub(crate) set_vars: HashSet<u32>,
    /// Type vars used with `Elems` / open `.toList` — List/Set/Map.
    pub(crate) elems_vars: HashSet<u32>,
    /// Type vars used with open `.take` / `.drop` / `.reverse` — List or String.
    pub(crate) take_vars: HashSet<u32>,
}

/// Lexical environment and mutability.
pub(crate) struct EnvState {
    pub(crate) env: Vec<HashMap<String, Scheme>>,
    /// Parallel to `env`: names bound with `var` (assignable) in that scope.
    pub(crate) mutables: Vec<HashSet<String>>,
}

impl Default for EnvState {
    fn default() -> Self {
        Self {
            env: vec![HashMap::default()],
            mutables: vec![HashSet::default()],
        }
    }
}

/// Trait / instance / UFCS resolution tables.
#[derive(Default)]
pub(crate) struct TraitState {
    pub(crate) ord_instances: HashSet<String>,
    pub(crate) num_instances: HashSet<String>,
    pub(crate) trait_vars: HashMap<u32, Vec<(String, String)>>,
    pub(crate) instances: HashSet<(String, String)>,
    pub(crate) trait_methods: HashMap<(String, String), Vec<String>>,
    pub(crate) method_trait: HashMap<String, String>,
    pub(crate) ufcs_rewrites: HashMap<lumia_syntax::Span, String>,
}

/// Product type field tables from HIR.
#[derive(Default)]
pub(crate) struct ProductState {
    pub(crate) products: HashMap<String, Vec<String>>,
    /// Sum ADT name → number of **parametric** payload slots (recursive spines
    /// like `S(n)` / `Cons(_, t)` do not allocate a param).
    pub(crate) sum_max_arity: HashMap<String, usize>,
    /// Sum variant name → (ADT name, arity, parametric-slot base offset).
    pub(crate) sum_ctors: HashMap<String, (String, usize, usize)>,
    /// Sum variant name → per-field kind (`true` = recursive self type).
    pub(crate) sum_field_recursive: HashMap<String, Vec<bool>>,
}

/// `return` / `alt` desugar bookkeeping.
#[derive(Default)]
pub(crate) struct AltReturnState {
    pub(crate) return_stack: Vec<Type>,
    pub(crate) alt_kinds: HashMap<lumia_syntax::Span, AltKind>,
    /// Ambiguous `.field` → (adt_name, idx) once receiver is known.
    pub(crate) product_field_rewrites: HashMap<lumia_syntax::Span, (String, i64)>,
    /// Deferred `with` → product type name.
    pub(crate) with_rewrites: HashMap<lumia_syntax::Span, String>,
}

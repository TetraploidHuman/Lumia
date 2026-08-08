//! Hindley-Milner style type inference + effect sets.

use lumia_hir::{Builtin, Expr, Fun, Item, Module};
use lumia_syntax::{BinOp, UnOp};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// Cross-file name visibility after import inlining (entry must not see `priv`).
#[derive(Debug, Clone, Default)]
pub struct NameVisibility {
    pub name_origin: HashMap<String, u32>,
    pub cross_file_visible: HashSet<String>,
    pub entry_file: u32,
    /// `import std.io.{println as log}` → local alias → builtin name.
    pub builtin_aliases: HashMap<String, String>,
}

impl NameVisibility {
    /// Entry module may only name locally declared or explicitly imported symbols.
    /// Dependency modules (inlined for linking) may use the full inlined namespace
    /// so public APIs can call their private/sibling helpers.
    pub fn allows(&self, name: &str, from_file: u32) -> bool {
        if self.name_origin.is_empty() {
            return true;
        }
        if from_file != self.entry_file {
            return true;
        }
        match self.name_origin.get(name) {
            Some(&origin) if origin == from_file => true,
            Some(_) => self.cross_file_visible.contains(name),
            None => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Float,
    Bool,
    String,
    Char,
    Unit,
    Fun(Vec<Type>, Box<Type>, Effect),
    Var(u32),
    /// List[T]
    List(Box<Type>),
    /// Map[K, V]
    Map(Box<Type>, Box<Type>),
    /// Set[T]
    Set(Box<Type>),
    /// Nominal sum type, e.g. Option[T] → Adt("Option", [T]).
    Adt { name: String, params: Vec<Type> },
    /// `(T1, T2, …)`
    Tuple(Vec<Type>),
}

/// Effect set ε — empty = pure; `Var` is open during inference (zonked to Pure if unconstrained).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effect {
    #[default]
    Pure,
    Io,
    Var(u32),
}

impl Effect {
    pub fn pure() -> Self {
        Self::Pure
    }
    pub fn io() -> Self {
        Self::Io
    }
    pub fn is_pure(self) -> bool {
        matches!(self, Self::Pure)
    }
    /// Concrete IO bit (unbound `Var` counts as not-yet-IO).
    pub fn has_io(self) -> bool {
        matches!(self, Self::Io)
    }
    /// Shallow lub without linking open vars. Inference uses `Infer::union_eff`
    /// so distinct `Var`s stay constrained together.
    pub fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Io, _) | (_, Self::Io) => Self::Io,
            (Self::Var(v), Self::Pure) | (Self::Pure, Self::Var(v)) => Self::Var(v),
            // Distinct open vars cannot be linked here; keep the first and rely on
            // `Infer::union_eff` at inference sites (see that method).
            (Self::Var(a), Self::Var(_)) => Self::Var(a),
            (Self::Pure, Self::Pure) => Self::Pure,
        }
    }
}

#[derive(Debug, Error)]
pub enum TypeError {
    #[error("{0}")]
    Message(String),
    #[error("{message}")]
    Located {
        span: lumia_syntax::Span,
        message: String,
    },
}

impl TypeError {
    pub fn span(&self) -> Option<lumia_syntax::Span> {
        match self {
            TypeError::Located { span, .. } => Some(*span),
            TypeError::Message(_) => None,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            TypeError::Message(m) | TypeError::Located { message: m, .. } => m,
        }
    }
}

fn at(span: lumia_syntax::Span, msg: impl Into<String>) -> TypeError {
    TypeError::Located {
        span,
        message: msg.into(),
    }
}

/// Source span for a HIR expression (walks into `Let`, which has no own span).
fn expr_span(e: &Expr) -> lumia_syntax::Span {
    match e {
        Expr::Int(_, s)
        | Expr::Float(_, s)
        | Expr::Bool(_, s)
        | Expr::String(_, s)
        | Expr::Char(_, s)
        | Expr::Unit(s)
        | Expr::Var(_, s)
        | Expr::Break(s)
        | Expr::Continue(s) => *s,
        Expr::Assign { span, .. }
        | Expr::Lambda { span, .. }
        | Expr::Call { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Unary { span, .. }
        | Expr::If { span, .. }
        | Expr::Loop { span, .. }
        | Expr::Seq { span, .. }
        | Expr::BuiltinCall { span, .. }
        | Expr::AdtNew { span, .. } => *span,
        Expr::Let { value, .. } => expr_span(value),
    }
}

fn locate(span: lumia_syntax::Span, err: TypeError) -> TypeError {
    match err {
        TypeError::Located { .. } => err,
        TypeError::Message(message) => TypeError::Located { span, message },
    }
}

#[derive(Debug, Clone)]
pub struct TypedModule {
    pub module: Module,
    pub fun_types: HashMap<String, Type>,
    pub main_effect: Effect,
    /// Expr span → pruned type (for LSP hover).
    pub type_at: Vec<(lumia_syntax::Span, Type)>,
    /// Top-level / local binding name → declaration span (for go-to-def).
    pub decls: HashMap<String, lumia_syntax::Span>,
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => write!(f, "Int"),
            Type::Float => write!(f, "Float"),
            Type::Bool => write!(f, "Bool"),
            Type::String => write!(f, "String"),
            Type::Char => write!(f, "Char"),
            Type::Unit => write!(f, "Unit"),
            Type::Var(v) => write!(f, "?{v}"),
            Type::List(t) => write!(f, "List[{t}]"),
            Type::Set(t) => write!(f, "Set[{t}]"),
            Type::Map(k, v) => write!(f, "Map[{k}, {v}]"),
            Type::Tuple(ts) => {
                write!(f, "(")?;
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{t}")?;
                }
                write!(f, ")")
            }
            Type::Adt { name, params } => {
                if params.is_empty() {
                    write!(f, "{name}")
                } else {
                    write!(f, "{name}[")?;
                    for (i, p) in params.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{p}")?;
                    }
                    write!(f, "]")
                }
            }
            Type::Fun(ps, r, e) => {
                write!(f, "(")?;
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                let eff = if e.has_io() { " / IO" } else { "" };
                write!(f, ") -> {r}{eff}")
            }
        }
    }
}

/// Hindley–Milner type scheme `∀ vars eff_vars. ty` (DESIGN §3.1 let-polymorphism).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scheme {
    vars: Vec<u32>,
    eff_vars: Vec<u32>,
    ty: Type,
}

impl Scheme {
    fn mono(ty: Type) -> Self {
        Self {
            vars: Vec::new(),
            eff_vars: Vec::new(),
            ty,
        }
    }
}

struct Infer {
    next_var: u32,
    next_eff: u32,
    subst: HashMap<u32, Type>,
    eff_subst: HashMap<u32, Effect>,
    env: Vec<HashMap<String, Scheme>>,
    /// Parallel to `env`: names bound with `var` (assignable) in that scope.
    mutables: Vec<HashSet<String>>,
    type_at: Vec<(lumia_syntax::Span, Type)>,
    decls: HashMap<String, lumia_syntax::Span>,
    vis: NameVisibility,
    /// File id of the function/val body currently being inferred.
    current_file: u32,
}

impl Infer {
    fn new(vis: NameVisibility) -> Self {
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
        }
    }

    fn check_name_visible(&self, name: &str, span: lumia_syntax::Span) -> Result<(), TypeError> {
        if self.vis.allows(name, self.current_file) {
            Ok(())
        } else {
            Err(at(
                span,
                format!("`{name}` is private or not imported into this module"),
            ))
        }
    }

    fn fresh(&mut self) -> Type {
        let v = self.next_var;
        self.next_var += 1;
        Type::Var(v)
    }

    fn fresh_eff(&mut self) -> Effect {
        let v = self.next_eff;
        self.next_eff += 1;
        Effect::Var(v)
    }

    fn push(&mut self) {
        self.env.push(HashMap::new());
        self.mutables.push(HashSet::new());
    }

    fn pop(&mut self) {
        self.env.pop();
        self.mutables.pop();
    }

    fn bind(&mut self, name: String, ty: Type) {
        self.bind_scheme(name, Scheme::mono(ty), false);
    }

    fn bind_mut(&mut self, name: String, ty: Type, mutable: bool) {
        self.bind_scheme(name, Scheme::mono(ty), mutable);
    }

    fn bind_scheme(&mut self, name: String, scheme: Scheme, mutable: bool) {
        self.env.last_mut().unwrap().insert(name.clone(), scheme);
        let m = self.mutables.last_mut().unwrap();
        if mutable {
            m.insert(name);
        } else {
            m.remove(&name);
        }
    }

    fn lookup(&mut self, name: &str) -> Option<Type> {
        let scheme = self.env.iter().rev().find_map(|scope| scope.get(name).cloned())?;
        Some(self.instantiate(&scheme))
    }

    /// True when the binding that `lookup` would see was introduced with `var`.
    fn is_mutable(&self, name: &str) -> bool {
        for (scope, muts) in self.env.iter().zip(self.mutables.iter()).rev() {
            if scope.contains_key(name) {
                return muts.contains(name);
            }
        }
        false
    }

    fn free_ty_vars(&mut self, ty: Type) -> HashSet<u32> {
        let ty = self.prune(ty);
        let mut acc = HashSet::new();
        self.collect_ty_vars(&ty, &mut acc);
        acc
    }

    fn collect_ty_vars(&mut self, ty: &Type, acc: &mut HashSet<u32>) {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.subst.get(v).cloned() {
                    let t = self.prune(t);
                    self.collect_ty_vars(&t, acc);
                } else {
                    acc.insert(*v);
                }
            }
            Type::Fun(ps, r, _) => {
                for p in ps {
                    self.collect_ty_vars(p, acc);
                }
                self.collect_ty_vars(r, acc);
            }
            Type::List(t) | Type::Set(t) => self.collect_ty_vars(t, acc),
            Type::Map(k, v) => {
                self.collect_ty_vars(k, acc);
                self.collect_ty_vars(v, acc);
            }
            Type::Adt { params, .. } => {
                for p in params {
                    self.collect_ty_vars(p, acc);
                }
            }
            Type::Tuple(ts) => {
                for t in ts {
                    self.collect_ty_vars(t, acc);
                }
            }
            _ => {}
        }
    }

    fn free_eff_vars_in_ty(&mut self, ty: Type) -> HashSet<u32> {
        let ty = self.prune(ty);
        let mut acc = HashSet::new();
        self.collect_eff_vars_in_ty(&ty, &mut acc);
        acc
    }

    fn collect_eff_vars_in_ty(&mut self, ty: &Type, acc: &mut HashSet<u32>) {
        match ty {
            Type::Fun(ps, r, e) => {
                for p in ps {
                    self.collect_eff_vars_in_ty(p, acc);
                }
                self.collect_eff_vars_in_ty(r, acc);
                match self.prune_eff(*e) {
                    Effect::Var(v) => {
                        acc.insert(v);
                    }
                    _ => {}
                }
            }
            Type::List(t) | Type::Set(t) => self.collect_eff_vars_in_ty(t, acc),
            Type::Map(k, v) => {
                self.collect_eff_vars_in_ty(k, acc);
                self.collect_eff_vars_in_ty(v, acc);
            }
            Type::Adt { params, .. } => {
                for p in params {
                    self.collect_eff_vars_in_ty(p, acc);
                }
            }
            Type::Tuple(ts) => {
                for t in ts {
                    self.collect_eff_vars_in_ty(t, acc);
                }
            }
            Type::Var(v) => {
                if let Some(t) = self.subst.get(v).cloned() {
                    let t = self.prune(t);
                    self.collect_eff_vars_in_ty(&t, acc);
                }
            }
            _ => {}
        }
    }

    fn env_free_ty_vars(&mut self) -> HashSet<u32> {
        let schemes: Vec<Scheme> = self
            .env
            .iter()
            .flat_map(|scope| scope.values().cloned())
            .collect();
        let mut acc = HashSet::new();
        for sch in schemes {
            let quantified: HashSet<u32> = sch.vars.iter().copied().collect();
            for v in self.free_ty_vars(sch.ty) {
                if !quantified.contains(&v) {
                    acc.insert(v);
                }
            }
        }
        acc
    }

    fn env_free_eff_vars(&mut self) -> HashSet<u32> {
        let schemes: Vec<Scheme> = self
            .env
            .iter()
            .flat_map(|scope| scope.values().cloned())
            .collect();
        let mut acc = HashSet::new();
        for sch in schemes {
            let quantified: HashSet<u32> = sch.eff_vars.iter().copied().collect();
            for v in self.free_eff_vars_in_ty(sch.ty) {
                if !quantified.contains(&v) {
                    acc.insert(v);
                }
            }
        }
        acc
    }

    fn generalize(&mut self, ty: Type) -> Scheme {
        let ty = self.prune(ty);
        let env_fvs = self.env_free_ty_vars();
        let env_efvs = self.env_free_eff_vars();
        let mut vars: Vec<u32> = self
            .free_ty_vars(ty.clone())
            .into_iter()
            .filter(|v| !env_fvs.contains(v))
            .collect();
        vars.sort_unstable();
        let mut eff_vars: Vec<u32> = self
            .free_eff_vars_in_ty(ty.clone())
            .into_iter()
            .filter(|v| !env_efvs.contains(v))
            .collect();
        eff_vars.sort_unstable();
        Scheme { vars, eff_vars, ty }
    }

    fn instantiate(&mut self, scheme: &Scheme) -> Type {
        let ty_map: HashMap<u32, Type> = scheme
            .vars
            .iter()
            .map(|&v| (v, self.fresh()))
            .collect();
        let eff_map: HashMap<u32, Effect> = scheme
            .eff_vars
            .iter()
            .map(|&v| (v, self.fresh_eff()))
            .collect();
        self.apply_scheme_subst(&scheme.ty, &ty_map, &eff_map)
    }

    fn apply_scheme_subst(
        &mut self,
        ty: &Type,
        ty_map: &HashMap<u32, Type>,
        eff_map: &HashMap<u32, Effect>,
    ) -> Type {
        match self.prune(ty.clone()) {
            Type::Var(v) => ty_map.get(&v).cloned().unwrap_or(Type::Var(v)),
            Type::Fun(ps, r, e) => {
                let e = match self.prune_eff(e) {
                    Effect::Var(v) => eff_map.get(&v).copied().unwrap_or(Effect::Var(v)),
                    other => other,
                };
                Type::Fun(
                    ps.iter()
                        .map(|p| self.apply_scheme_subst(p, ty_map, eff_map))
                        .collect(),
                    Box::new(self.apply_scheme_subst(&r, ty_map, eff_map)),
                    e,
                )
            }
            Type::List(t) => Type::List(Box::new(self.apply_scheme_subst(&t, ty_map, eff_map))),
            Type::Set(t) => Type::Set(Box::new(self.apply_scheme_subst(&t, ty_map, eff_map))),
            Type::Map(k, v) => Type::Map(
                Box::new(self.apply_scheme_subst(&k, ty_map, eff_map)),
                Box::new(self.apply_scheme_subst(&v, ty_map, eff_map)),
            ),
            Type::Adt { name, params } => Type::Adt {
                name,
                params: params
                    .iter()
                    .map(|p| self.apply_scheme_subst(p, ty_map, eff_map))
                    .collect(),
            },
            Type::Tuple(ts) => Type::Tuple(
                ts.iter()
                    .map(|t| self.apply_scheme_subst(t, ty_map, eff_map))
                    .collect(),
            ),
            other => other,
        }
    }

    fn prune(&mut self, ty: Type) -> Type {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.subst.get(&v).cloned() {
                    let t = self.prune(t);
                    self.subst.insert(v, t.clone());
                    t
                } else {
                    Type::Var(v)
                }
            }
            Type::Fun(ps, r, e) => Type::Fun(
                ps.into_iter().map(|p| self.prune(p)).collect(),
                Box::new(self.prune(*r)),
                self.prune_eff(e),
            ),
            Type::List(t) => Type::List(Box::new(self.prune(*t))),
            Type::Map(k, v) => Type::Map(Box::new(self.prune(*k)), Box::new(self.prune(*v))),
            Type::Set(t) => Type::Set(Box::new(self.prune(*t))),
            other => other,
        }
    }

    fn prune_eff(&mut self, e: Effect) -> Effect {
        match e {
            Effect::Var(v) => {
                if let Some(e2) = self.eff_subst.get(&v).cloned() {
                    let e2 = self.prune_eff(e2);
                    self.eff_subst.insert(v, e2);
                    e2
                } else {
                    Effect::Var(v)
                }
            }
            other => other,
        }
    }

    /// Unbound effect vars become Pure (generalize as pure when unconstrained).
    fn zonk_eff(&mut self, e: Effect) -> Effect {
        match self.prune_eff(e) {
            Effect::Var(_) => Effect::Pure,
            other => other,
        }
    }

    fn zonk_type(&mut self, ty: Type) -> Type {
        match self.prune(ty) {
            Type::Fun(ps, r, e) => Type::Fun(
                ps.into_iter().map(|p| self.zonk_type(p)).collect(),
                Box::new(self.zonk_type(*r)),
                self.zonk_eff(e),
            ),
            Type::List(t) => Type::List(Box::new(self.zonk_type(*t))),
            Type::Map(k, v) => Type::Map(Box::new(self.zonk_type(*k)), Box::new(self.zonk_type(*v))),
            Type::Set(t) => Type::Set(Box::new(self.zonk_type(*t))),
            Type::Adt { name, params } => Type::Adt {
                name,
                params: params.into_iter().map(|p| self.zonk_type(p)).collect(),
            },
            Type::Tuple(ts) => Type::Tuple(ts.into_iter().map(|t| self.zonk_type(t)).collect()),
            other => other,
        }
    }

    /// Least upper bound of effects, linking distinct open vars so either becoming
    /// `Io` zonks both (needed for `f(x); g(x)` HOF bodies).
    fn union_eff(&mut self, a: Effect, b: Effect) -> Effect {
        let a = self.prune_eff(a);
        let b = self.prune_eff(b);
        match (a, b) {
            (Effect::Io, _) | (_, Effect::Io) => Effect::Io,
            (Effect::Pure, Effect::Pure) => Effect::Pure,
            (Effect::Var(v), Effect::Pure) | (Effect::Pure, Effect::Var(v)) => Effect::Var(v),
            (Effect::Var(a), Effect::Var(b)) => {
                if a != b {
                    self.eff_subst.insert(a, Effect::Var(b));
                }
                Effect::Var(b)
            }
        }
    }

    fn union3_eff(&mut self, a: Effect, b: Effect, c: Effect) -> Effect {
        let ab = self.union_eff(a, b);
        self.union_eff(ab, c)
    }

    /// Unify effects for equality. `Pure` and `Io` are distinct (do **not** unify).
    /// Open `Var` stays flexible when matched with `Pure` so a later `Io` use can
    /// still instantiate it (HOF effect polymorphism); matching `Io` binds the var.
    fn unify_eff(&mut self, a: Effect, b: Effect) -> Result<(), TypeError> {
        let a = self.prune_eff(a);
        let b = self.prune_eff(b);
        match (a, b) {
            (Effect::Pure, Effect::Pure) | (Effect::Io, Effect::Io) => Ok(()),
            (Effect::Pure, Effect::Io) | (Effect::Io, Effect::Pure) => Err(TypeError::Message(
                "effect mismatch: cannot unify Pure with Io".into(),
            )),
            (Effect::Var(v), Effect::Pure) | (Effect::Pure, Effect::Var(v)) => {
                let _ = v;
                Ok(())
            }
            (Effect::Var(v), Effect::Io) | (Effect::Io, Effect::Var(v)) => {
                self.eff_subst.insert(v, Effect::Io);
                Ok(())
            }
            (Effect::Var(v), Effect::Var(w)) => {
                if v != w {
                    self.eff_subst.insert(v, Effect::Var(w));
                }
                Ok(())
            }
        }
    }

    /// Join types at merge points (`if` arms, `var` assign). Function effects use
    /// lub (`Pure ⊔ Io = Io`) instead of equality so IO cannot be lost.
    fn join_types(
        &mut self,
        a: Type,
        b: Type,
        span: lumia_syntax::Span,
    ) -> Result<Type, TypeError> {
        let a = self.prune(a);
        let b = self.prune(b);
        match (a, b) {
            (Type::Fun(aps, ar, ae), Type::Fun(bps, br, be)) => {
                if aps.len() != bps.len() {
                    return Err(at(span, "function arity mismatch"));
                }
                let mut ps = Vec::with_capacity(aps.len());
                for (x, y) in aps.into_iter().zip(bps) {
                    self.unify_at(span, x.clone(), y)?;
                    ps.push(self.prune(x));
                }
                let r = self.join_types(*ar, *br, span)?;
                let e = self.union_eff(ae, be);
                Ok(Type::Fun(ps, Box::new(r), e))
            }
            (Type::List(a), Type::List(b)) => {
                Ok(Type::List(Box::new(self.join_types(*a, *b, span)?)))
            }
            (Type::Set(a), Type::Set(b)) => {
                Ok(Type::Set(Box::new(self.join_types(*a, *b, span)?)))
            }
            (Type::Map(ak, av), Type::Map(bk, bv)) => Ok(Type::Map(
                Box::new(self.join_types(*ak, *bk, span)?),
                Box::new(self.join_types(*av, *bv, span)?),
            )),
            (Type::Tuple(a), Type::Tuple(b)) => {
                if a.len() != b.len() {
                    return Err(at(span, "tuple arity mismatch"));
                }
                let mut ts = Vec::with_capacity(a.len());
                for (x, y) in a.into_iter().zip(b) {
                    ts.push(self.join_types(x, y, span)?);
                }
                Ok(Type::Tuple(ts))
            }
            (a, b) => {
                self.unify_at(span, a.clone(), b)?;
                Ok(self.prune(a))
            }
        }
    }

    fn rebind(&mut self, name: &str, ty: Type) -> Result<(), TypeError> {
        self.rebind_scheme(name, Scheme::mono(ty))
    }

    fn rebind_scheme(&mut self, name: &str, scheme: Scheme) -> Result<(), TypeError> {
        for scope in self.env.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), scheme);
                return Ok(());
            }
        }
        Err(TypeError::Message(format!("unbound `{name}` in rebind")))
    }

    fn unify(&mut self, a: Type, b: Type) -> Result<(), TypeError> {
        let a = self.prune(a);
        let b = self.prune(b);
        match (a, b) {
            (Type::Var(v), t) | (t, Type::Var(v)) => {
                if occurs(v, &t) {
                    return Err(TypeError::Message("infinite type".into()));
                }
                self.subst.insert(v, t);
                Ok(())
            }
            (Type::Int, Type::Int)
            | (Type::Float, Type::Float)
            | (Type::Bool, Type::Bool)
            | (Type::String, Type::String)
            | (Type::Char, Type::Char)
            | (Type::Unit, Type::Unit) => Ok(()),
            (Type::List(a), Type::List(b)) => self.unify(*a, *b),
            (Type::Set(a), Type::Set(b)) => self.unify(*a, *b),
            (Type::Map(ak, av), Type::Map(bk, bv)) => {
                self.unify(*ak, *bk)?;
                self.unify(*av, *bv)
            }
            (
                Type::Adt {
                    name: a,
                    params: ap,
                },
                Type::Adt {
                    name: b,
                    params: bp,
                },
            ) => {
                if a != b || ap.len() != bp.len() {
                    return Err(TypeError::Message(format!(
                        "type mismatch: Adt({a}) vs Adt({b})"
                    )));
                }
                for (x, y) in ap.into_iter().zip(bp) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Type::Tuple(a), Type::Tuple(b)) => {
                if a.len() != b.len() {
                    return Err(TypeError::Message("tuple arity mismatch".into()));
                }
                for (x, y) in a.into_iter().zip(b) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Type::Fun(a_ps, a_r, a_e), Type::Fun(b_ps, b_r, b_e)) => {
                if a_ps.len() != b_ps.len() {
                    return Err(TypeError::Message("function arity mismatch".into()));
                }
                for (x, y) in a_ps.into_iter().zip(b_ps) {
                    self.unify(x, y)?;
                }
                self.unify(*a_r, *b_r)?;
                self.unify_eff(a_e, b_e)
            }
            (a, b) => Err(TypeError::Message(format!(
                "type mismatch: {a:?} vs {b:?}"
            ))),
        }
    }

    fn unify_at(
        &mut self,
        span: lumia_syntax::Span,
        a: Type,
        b: Type,
    ) -> Result<(), TypeError> {
        self.unify(a, b).map_err(|e| locate(span, e))
    }

    fn infer_expr(&mut self, expr: &Expr) -> Result<(Type, Effect), TypeError> {
        let (t, e) = self.infer_expr_inner(expr)?;
        self.type_at.push((expr_span(expr), t.clone()));
        Ok((t, e))
    }

    fn infer_expr_inner(&mut self, expr: &Expr) -> Result<(Type, Effect), TypeError> {
        match expr {
            Expr::Int(_, _) => Ok((Type::Int, Effect::pure())),
            Expr::Float(_, _) => Ok((Type::Float, Effect::pure())),
            Expr::Bool(_, _) => Ok((Type::Bool, Effect::pure())),
            Expr::String(_, _) => Ok((Type::String, Effect::pure())),
            Expr::Char(_, _) => Ok((Type::Char, Effect::pure())),
            Expr::Unit(_) => Ok((Type::Unit, Effect::pure())),
            Expr::Var(name, span) => {
                let t = self
                    .lookup(name)
                    .ok_or_else(|| at(*span, format!("unbound variable `{name}`")))?;
                self.check_name_visible(name, *span)?;
                Ok((t, Effect::pure()))
            }
            Expr::Let {
                name,
                value,
                body,
                mutable,
                ..
            } => {
                let (vt, ve) = self.infer_expr(value)?;
                self.push();
                // Immutable lets generalize (HM let-poly); `var` stays monomorphic.
                if *mutable {
                    self.bind_mut(name.clone(), vt, true);
                } else {
                    let scheme = self.generalize(vt);
                    self.bind_scheme(name.clone(), scheme, false);
                }
                let (bt, be) = self.infer_expr(body)?;
                self.pop();
                Ok((bt, self.union_eff(ve, be)))
            }
            Expr::Assign { name, value, span } => {
                let expect = self.lookup(name).ok_or_else(|| at(*span, format!("unbound `{name}` in assign")))?;
                if !self.is_mutable(name) {
                    return Err(at(
                        *span,
                        format!("cannot assign to immutable binding `{name}` (use `var`)"),
                    ));
                }
                let (vt, ve) = self.infer_expr(value)?;
                // Widen Fun effects (Pure ⊔ Io = Io) and update the binding so
                // later calls see the lub — equality unify would reject or, with
                // the old Pure↔Io hole, silently keep Pure.
                let joined = self.join_types(expect, vt, *span)?;
                self.rebind(name, joined)?;
                Ok((Type::Unit, ve))
            }
            Expr::Lambda { params, body, .. } => {
                self.push();
                let mut pts = vec![];
                for p in params {
                    let tv = self.fresh();
                    pts.push(tv.clone());
                    self.bind(p.clone(), tv);
                }
                let (rt, re) = self.infer_expr(body)?;
                self.pop();
                Ok((Type::Fun(pts, Box::new(rt), re), Effect::pure()))
            }
            Expr::Call { callee, args, span } => {
                // Special-case listOf(...): List[T] with unified element type
                if let Expr::Var(name, _) = callee.as_ref() {
                    if name == "listOf" {
                        let mut aes = Effect::pure();
                        let elem = self.fresh();
                        for a in args {
                            let (t, e) = self.infer_expr(a)?;
                            aes = self.union_eff(aes, e);
                            self.unify_at(*span, elem.clone(), t)?;
                        }
                        return Ok((Type::List(Box::new(self.prune(elem))), aes));
                    }
                    if name == "setOf" {
                        let mut aes = Effect::pure();
                        let elem = self.fresh();
                        for a in args {
                            let (t, e) = self.infer_expr(a)?;
                            aes = self.union_eff(aes, e);
                            self.unify_at(*span, elem.clone(), t)?;
                        }
                        return Ok((Type::Set(Box::new(self.prune(elem))), aes));
                    }
                    if name == "mapOf" {
                        let mut aes = Effect::pure();
                        let k = self.fresh();
                        let v = self.fresh();
                        if args.len() % 2 != 0 {
                            return Err(at(*span, 
                                "mapOf expects an even number of key/value arguments",
                            ));
                        }
                        for chunk in args.chunks(2) {
                            let (kt, ke) = self.infer_expr(&chunk[0])?;
                            let (vt, ve) = self.infer_expr(&chunk[1])?;
                            aes = self.union3_eff(aes, ke, ve);
                            self.unify_at(*span, k.clone(), kt)?;
                            self.unify_at(*span, v.clone(), vt)?;
                        }
                        return Ok((
                            Type::Map(Box::new(self.prune(k)), Box::new(self.prune(v))),
                            aes,
                        ));
                    }
                }
                let (ct, ce) = self.infer_expr(callee)?;
                let mut aes = Effect::pure();
                let mut ats = vec![];
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    ats.push(t);
                    aes = self.union_eff(aes, e);
                }
                let ret = self.fresh();
                // Open effect when callee is not yet a concrete Fun — allows HOFs to
                // pick up IO from effectful callbacks (Var stays open vs Pure; Io binds).
                let call_eff = match self.prune(ct.clone()) {
                    Type::Fun(_, _, e) => e,
                    _ => self.fresh_eff(),
                };
                self.unify_at(*span, 
                    ct,
                    Type::Fun(ats, Box::new(ret.clone()), call_eff),
                )?;
                let fun_eff = self.prune_eff(call_eff);
                Ok((self.prune(ret), self.union3_eff(ce, aes, fun_eff)))
            }
            Expr::BuiltinCall { name, args, span } => match name {
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr => {
                    if args.len() != 1 {
                        return Err(at(*span, "println takes 1 argument"));
                    }
                    let (t, e) = self.infer_expr(&args[0])?;
                    let t = self.prune(t);
                    match t {
                        Type::Int | Type::String | Type::Bool | Type::Float | Type::Char => {}
                        Type::Var(_) => {
                            return Err(at(
                                *span,
                                "println: cannot resolve argument type (annotate or use a concrete value)",
                            ));
                        }
                        other => {
                            return Err(at(*span, format!(
                                "println: unsupported type {other:?}"
                            )));
                        }
                    }
                    Ok((Type::Unit, self.union_eff(Effect::io(), e)))
                }
                Builtin::ListLen => {
                    if args.len() != 1 {
                        return Err(at(*span, "len takes 1 argument"));
                    }
                    let (t, e) = self.infer_expr(&args[0])?;
                    let t = self.prune(t);
                    match t {
                        Type::List(_) | Type::Set(_) | Type::Map(_, _) | Type::String => {}
                        Type::Var(_) => {
                            // Unconstrained: treat as List (match desugar / polymorphic use).
                            let elem = self.fresh();
                            self.unify_at(*span, t, Type::List(Box::new(elem)))?;
                        }
                        other => {
                            return Err(at(*span, format!(
                                "len: expected List/Set/Map/String, got {other:?}"
                            )));
                        }
                    }
                    Ok((Type::Int, e))
                }
                Builtin::ListGet => {
                    if args.len() != 2 {
                        return Err(at(*span, "get takes 2 arguments"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (it, ie) = self.infer_expr(&args[1])?;
                    let elem = match self.prune(lt.clone()) {
                        Type::List(t) => {
                            self.unify_at(*span, it, Type::Int)?;
                            *t
                        }
                        Type::Set(t) => {
                            self.unify_at(*span, it, Type::Int)?;
                            *t
                        }
                        Type::Map(k, v) => {
                            self.unify_at(*span, it, *k)?;
                            Type::Adt {
                                name: "Option".into(),
                                params: vec![*v],
                            }
                        }
                        Type::Var(_) => {
                            // Default to List (match desugar); Map is typed from mapOf.
                            self.unify_at(*span, it, Type::Int)?;
                            let elem = self.fresh();
                            self.unify_at(*span, lt, Type::List(Box::new(elem.clone())))?;
                            elem
                        }
                        other => {
                            return Err(at(*span, format!(
                                "get: expected List, Set, or Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((elem, self.union_eff(le, ie)))
                }
                Builtin::Contains => {
                    if args.len() != 2 {
                        return Err(at(*span, "contains takes 2 arguments"));
                    }
                    let (ct, ce) = self.infer_expr(&args[0])?;
                    let (kt, ke) = self.infer_expr(&args[1])?;
                    match self.prune(ct.clone()) {
                        Type::Map(k, _) => self.unify_at(*span, kt, *k)?,
                        Type::Set(e) => self.unify_at(*span, kt, *e)?,
                        Type::String => self.unify_at(*span, kt, Type::String)?,
                        Type::Var(_) => {
                            // Leave open so later use can unify with Set/Map/String.
                        }
                        other => {
                            return Err(at(*span, format!(
                                "contains: expected Map, Set, or String, got {other:?}"
                            )));
                        }
                    }
                    Ok((Type::Bool, self.union_eff(ce, ke)))
                }
                Builtin::MapSet => {
                    if args.len() != 3 {
                        return Err(at(*span, 
                            "set takes 3 arguments (map/list, key/index, value)",
                        ));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let (kt, ke) = self.infer_expr(&args[1])?;
                    let (vt, ve) = self.infer_expr(&args[2])?;
                    match self.prune(mt.clone()) {
                        Type::Map(k, v) => {
                            self.unify_at(*span, kt, *k.clone())?;
                            self.unify_at(*span, vt, *v.clone())?;
                            Ok((Type::Map(k, v), self.union3_eff(me, ke, ve)))
                        }
                        Type::List(elem) => {
                            self.unify_at(*span, kt, Type::Int)?;
                            self.unify_at(*span, vt, *elem.clone())?;
                            Ok((Type::List(elem), self.union3_eff(me, ke, ve)))
                        }
                        Type::Var(_) => {
                            // Prefer Map when unconstrained (UFCS `.set` on maps).
                            self.unify_at(*span, 
                                mt,
                                Type::Map(Box::new(kt.clone()), Box::new(vt.clone())),
                            )?;
                            Ok((
                                Type::Map(Box::new(kt), Box::new(vt)),
                                self.union3_eff(me, ke, ve),
                            ))
                        }
                        other => Err(at(*span, format!(
                            "set: expected Map or List, got {other:?}"
                        ))),
                    }
                }
                Builtin::MapRemove => {
                    if args.len() != 2 {
                        return Err(at(*span, "remove takes 2 arguments"));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let (kt, ke) = self.infer_expr(&args[1])?;
                    match self.prune(mt.clone()) {
                        Type::Map(k, v) => {
                            self.unify_at(*span, kt, *k.clone())?;
                            Ok((Type::Map(k, v), self.union_eff(me, ke)))
                        }
                        Type::Set(e) => {
                            self.unify_at(*span, kt, *e.clone())?;
                            Ok((Type::Set(e), self.union_eff(me, ke)))
                        }
                        Type::Var(_) => {
                            // Keep open; call site / later use constrains Map vs Set.
                            Ok((mt, self.union_eff(me, ke)))
                        }
                        other => Err(at(*span, format!(
                            "remove: expected Map or Set, got {other:?}"
                        ))),
                    }
                }
                Builtin::SetInsert => {
                    if args.len() != 2 {
                        return Err(at(*span, "insert takes 2 arguments"));
                    }
                    let (st, se) = self.infer_expr(&args[0])?;
                    let (et, ee) = self.infer_expr(&args[1])?;
                    match self.prune(st.clone()) {
                        Type::Set(e) => {
                            self.unify_at(*span, et, *e.clone())?;
                            Ok((Type::Set(e), self.union_eff(se, ee)))
                        }
                        Type::Var(_) => {
                            self.unify_at(*span, st, Type::Set(Box::new(et.clone())))?;
                            Ok((Type::Set(Box::new(et)), self.union_eff(se, ee)))
                        }
                        other => Err(at(*span, format!(
                            "insert: expected Set, got {other:?}"
                        ))),
                    }
                }
                Builtin::Elems => {
                    if args.len() != 1 {
                        return Err(at(*span, "elems takes 1 argument"));
                    }
                    let (ct, ce) = self.infer_expr(&args[0])?;
                    let list_ty = match self.prune(ct.clone()) {
                        Type::List(e) => Type::List(e),
                        Type::Set(e) => Type::List(e),
                        Type::Map(k, _) => Type::List(k),
                        Type::Var(_) => {
                            let e = self.fresh();
                            self.unify_at(*span, ct, Type::List(Box::new(e.clone())))?;
                            Type::List(Box::new(e))
                        }
                        other => {
                            return Err(at(
                                *span,
                                format!("elems: expected List, Set, or Map, got {other:?}"),
                            ));
                        }
                    };
                    Ok((list_ty, ce))
                }
                Builtin::MapKeys => {
                    if args.len() != 1 {
                        return Err(at(*span, "keys takes 1 argument"));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let k = match self.prune(mt.clone()) {
                        Type::Map(k, _) => *k,
                        Type::Var(_) => {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify_at(*span, mt, Type::Map(Box::new(k.clone()), Box::new(v)))?;
                            k
                        }
                        other => {
                            return Err(at(*span, format!(
                                "keys: expected Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(Box::new(k)), me))
                }
                Builtin::MapValues => {
                    if args.len() != 1 {
                        return Err(at(*span, "values takes 1 argument"));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    let v = match self.prune(mt.clone()) {
                        Type::Map(_, v) => *v,
                        Type::Var(_) => {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify_at(*span, mt, Type::Map(Box::new(k), Box::new(v.clone())))?;
                            v
                        }
                        other => {
                            return Err(at(*span, format!(
                                "values: expected Map, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(Box::new(v)), me))
                }
                Builtin::MapItems => {
                    if args.len() != 1 {
                        return Err(at(*span, "items takes 1 argument"));
                    }
                    let (mt, me) = self.infer_expr(&args[0])?;
                    // Map → List[(K,V)]; already a List of pairs → identity (for-in sugar).
                    let pair_list = match self.prune(mt.clone()) {
                        Type::Map(k, v) => Type::List(Box::new(Type::Tuple(vec![*k, *v]))),
                        Type::List(elem) => {
                            let elem = self.prune(*elem);
                            match elem {
                                Type::Tuple(ts) if ts.len() == 2 => {
                                    Type::List(Box::new(Type::Tuple(ts)))
                                }
                                Type::Adt { name, params }
                                    if (name == "__Tuple" || name.is_empty())
                                        && params.len() == 2 =>
                                {
                                    Type::List(Box::new(Type::Tuple(params)))
                                }
                                Type::Var(_) => {
                                    let k = self.fresh();
                                    let v = self.fresh();
                                    let pair = Type::Tuple(vec![k, v]);
                                    self.unify_at(
                                        *span,
                                        Type::List(Box::new(elem)),
                                        Type::List(Box::new(pair.clone())),
                                    )?;
                                    Type::List(Box::new(pair))
                                }
                                other => {
                                    return Err(at(
                                        *span,
                                        format!(
                                            "items: expected Map or List of pairs, got List({other:?})"
                                        ),
                                    ));
                                }
                            }
                        }
                        Type::Var(_) => {
                            let k = self.fresh();
                            let v = self.fresh();
                            self.unify_at(
                                *span,
                                mt,
                                Type::Map(Box::new(k.clone()), Box::new(v.clone())),
                            )?;
                            Type::List(Box::new(Type::Tuple(vec![k, v])))
                        }
                        other => {
                            return Err(at(
                                *span,
                                format!("items: expected Map or List of pairs, got {other:?}"),
                            ));
                        }
                    };
                    Ok((pair_list, me))
                }
                Builtin::AdtTag => {
                    if args.len() != 1 {
                        return Err(at(*span, "adt_tag takes 1 argument"));
                    }
                    let (_, e) = self.infer_expr(&args[0])?;
                    Ok((Type::Int, e))
                }
                Builtin::AdtField => {
                    // 2 args: tuple/positional `.0`; 3 args: product field with expected ADT name.
                    if args.len() != 2 && args.len() != 3 {
                        return Err(at(*span, "adt_field takes 2 or 3 arguments"));
                    }
                    let (recv_ty, ae) = self.infer_expr(&args[0])?;
                    let (it, ie) = self.infer_expr(&args[1])?;
                    self.unify_at(*span, it, Type::Int)?;
                    let mut eff = self.union_eff(ae, ie);
                    let expect_adt = if args.len() == 3 {
                        let (nt, ne) = self.infer_expr(&args[2])?;
                        self.unify_at(*span, nt, Type::String)?;
                        eff = self.union_eff(eff, ne);
                        match &args[2] {
                            Expr::String(s, _) => Some(s.as_str()),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let idx = match &args[1] {
                        Expr::Int(n, _) if *n >= 0 => *n as usize,
                        _ => {
                            return Err(at(*span, "adt_field index must be a non-negative literal"));
                        }
                    };
                    let elem = match self.prune(recv_ty.clone()) {
                        Type::Adt { name, params } => {
                            if let Some(want) = expect_adt {
                                // Variant patterns pass ctor name (`Ok`/`Err`/`Some`);
                                // product patterns / field proj pass the ADT name.
                                if name == "Result" && (want == "Ok" || want == "Err") {
                                    if idx != 0 {
                                        return Err(at(
                                            *span,
                                            format!(
                                                "Result::{want} has a single payload (index 0), got {idx}"
                                            ),
                                        ));
                                    }
                                    let pi = if want == "Ok" { 0 } else { 1 };
                                    return Ok((
                                        params.get(pi).cloned().ok_or_else(|| {
                                            at(
                                                *span,
                                                format!(
                                                    "Result::{want} payload missing (params {})",
                                                    params.len()
                                                ),
                                            )
                                        })?,
                                        eff,
                                    ));
                                }
                                if name == "Option" && want == "Some" {
                                    if idx != 0 {
                                        return Err(at(
                                            *span,
                                            format!(
                                                "Option::Some has a single payload (index 0), got {idx}"
                                            ),
                                        ));
                                    }
                                    return Ok((
                                        params.first().cloned().ok_or_else(|| {
                                            at(*span, "Option::Some payload missing")
                                        })?,
                                        eff,
                                    ));
                                }
                                if name != want {
                                    return Err(at(
                                        *span,
                                        format!(
                                            "field projection expects type `{want}`, got `{name}`"
                                        ),
                                    ));
                                }
                            }
                            params.get(idx).cloned().ok_or_else(|| {
                                at(
                                    *span,
                                    format!(
                                        "field index {idx} out of range for `{name}` (arity {})",
                                        params.len()
                                    ),
                                )
                            })?
                        }
                        Type::Tuple(ts) => {
                            if expect_adt.is_some() {
                                return Err(at(
                                    *span,
                                    "named product field applied to a tuple",
                                ));
                            }
                            ts.get(idx).cloned().ok_or_else(|| {
                                at(
                                    *span,
                                    format!(
                                        "tuple index {idx} out of range (arity {})",
                                        ts.len()
                                    ),
                                )
                            })?
                        }
                        Type::Var(_) => {
                            // Receiver still open (match desugar / inference order).
                            // Concrete Adt/Tuple checks apply once the type is known.
                            self.fresh()
                        }
                        other => {
                            return Err(at(
                                *span,
                                format!("field projection: expected product/tuple, got {other:?}"),
                            ));
                        }
                    };
                    Ok((elem, eff))
                }
                Builtin::ListSlice => {
                    if args.len() != 2 {
                        return Err(at(*span, "slice/drop takes 2 arguments"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (it, ie) = self.infer_expr(&args[1])?;
                    self.unify_at(*span, it, Type::Int)?;
                    let elem = match self.prune(lt.clone()) {
                        Type::List(t) => t,
                        Type::Var(_) => {
                            let elem = self.fresh();
                            self.unify_at(*span, lt, Type::List(Box::new(elem.clone())))?;
                            Box::new(elem)
                        }
                        other => {
                            return Err(at(*span, format!(
                                "slice/drop: expected List, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(elem), self.union_eff(le, ie)))
                }
                Builtin::ListTake => {
                    if args.len() != 2 {
                        return Err(at(*span, "take takes 2 arguments"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (it, ie) = self.infer_expr(&args[1])?;
                    self.unify_at(*span, it, Type::Int)?;
                    let elem = match self.prune(lt.clone()) {
                        Type::List(t) => t,
                        Type::Var(_) => {
                            let elem = self.fresh();
                            self.unify_at(*span, lt, Type::List(Box::new(elem.clone())))?;
                            Box::new(elem)
                        }
                        other => {
                            return Err(at(*span, format!(
                                "take: expected List, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(elem), self.union_eff(le, ie)))
                }
                Builtin::ListReverse => {
                    if args.len() != 1 {
                        return Err(at(*span, "reverse takes 1 argument"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let elem = match self.prune(lt.clone()) {
                        Type::List(t) => t,
                        Type::Var(_) => {
                            let elem = self.fresh();
                            self.unify_at(*span, lt, Type::List(Box::new(elem.clone())))?;
                            Box::new(elem)
                        }
                        other => {
                            return Err(at(*span, format!(
                                "reverse: expected List, got {other:?}"
                            )));
                        }
                    };
                    Ok((Type::List(elem), le))
                }
                Builtin::ListSort => {
                    if args.len() != 1 {
                        return Err(at(*span, "sort takes 1 argument"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    match self.prune(lt.clone()) {
                        Type::List(t) => {
                            self.unify_at(*span, *t, Type::Int)?;
                        }
                        Type::Var(_) => {
                            self.unify_at(*span, lt, Type::List(Box::new(Type::Int)))?;
                        }
                        other => {
                            return Err(at(*span, format!(
                                "sort: expected List[Int], got {other:?}"
                            )));
                        }
                    }
                    Ok((Type::List(Box::new(Type::Int)), le))
                }
                Builtin::ListSortByKeys => {
                    if args.len() != 2 {
                        return Err(at(*span, 
                            "sortByKeys takes 2 arguments (values, keys)",
                        ));
                    }
                    let (vt, ve) = self.infer_expr(&args[0])?;
                    let (kt, ke) = self.infer_expr(&args[1])?;
                    let elem = match self.prune(vt.clone()) {
                        Type::List(t) => *t,
                        Type::Var(_) => {
                            let e = self.fresh();
                            self.unify_at(*span, vt, Type::List(Box::new(e.clone())))?;
                            e
                        }
                        other => {
                            return Err(at(*span, format!(
                                "sortBy: expected List, got {other:?}"
                            )));
                        }
                    };
                    match self.prune(kt.clone()) {
                        Type::List(t) => {
                            let t = self.prune(*t);
                            match t {
                                Type::Int | Type::String | Type::Char => {}
                                Type::Var(_) => {}
                                other => {
                                    return Err(at(*span, format!(
                                        "sortBy keys: expected List[Int|String|Char], got List[{other:?}]"
                                    )));
                                }
                            }
                        }
                        Type::Var(_) => {
                            // Key type filled by the key function; leave open.
                        }
                        other => {
                            return Err(at(*span, format!(
                                "sortBy keys: expected List, got {other:?}"
                            )));
                        }
                    }
                    Ok((Type::List(Box::new(elem)), self.union_eff(ve, ke)))
                }
                Builtin::ListParMap => {
                    if args.len() != 2 {
                        return Err(at(*span, "par map takes 2 arguments"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (ft, fe) = self.infer_expr(&args[1])?;
                    let elem = match self.prune(lt.clone()) {
                        Type::List(t) => *t,
                        Type::Var(_) => {
                            let e = self.fresh();
                            self.unify_at(*span, lt, Type::List(Box::new(e.clone())))?;
                            e
                        }
                        other => {
                            return Err(at(*span, format!("map: expected List, got {other:?}")));
                        }
                    };
                    // Parallel workers use TLS heaps — require *concrete* scalar
                    // Int/Bool/Float (reject open Vars that could later be heap types).
                    let elem = self.prune(elem);
                    match &elem {
                        Type::Int | Type::Bool | Type::Float => {}
                        other => {
                            return Err(at(
                                *span,
                                format!(
                                    "parallel map: element type must be Int/Bool/Float (got {other:?}); omit --parallel for heap maps"
                                ),
                            ));
                        }
                    }
                    let out = self.fresh();
                    // Require a pure callback: concrete Io is rejected; open Vars
                    // unify with Pure (stay flexible) then zonk unconstrained → Pure.
                    let cb_eff = match self.prune(ft.clone()) {
                        Type::Fun(_, _, e) => self.prune_eff(e),
                        _ => Effect::pure(),
                    };
                    if cb_eff.has_io() {
                        return Err(at(
                            *span,
                            "parallel map: callback must be pure (no I/O); omit --parallel for effectful maps",
                        ));
                    }
                    self.unify_at(
                        *span,
                        ft,
                        Type::Fun(
                            vec![elem],
                            Box::new(out.clone()),
                            Effect::pure(),
                        ),
                    )?;
                    let out = self.prune(out);
                    match &out {
                        Type::Int | Type::Bool | Type::Float => {}
                        other => {
                            return Err(at(
                                *span,
                                format!(
                                    "parallel map: result type must be Int/Bool/Float (got {other:?}); omit --parallel for heap maps"
                                ),
                            ));
                        }
                    }
                    Ok((Type::List(Box::new(out)), self.union_eff(le, fe)))
                }
                Builtin::ListJoin => {
                    if args.len() != 2 {
                        return Err(at(*span, 
                            "join takes 2 arguments (list, separator)",
                        ));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (st, se) = self.infer_expr(&args[1])?;
                    self.unify_at(*span, st, Type::String)?;
                    match self.prune(lt.clone()) {
                        Type::List(t) => self.unify_at(*span, *t, Type::String)?,
                        Type::Var(_) => {
                            self.unify_at(*span, lt, Type::List(Box::new(Type::String)))?;
                        }
                        other => {
                            return Err(at(*span, format!(
                                "join: expected List[String], got {other:?}"
                            )));
                        }
                    }
                    Ok((Type::String, self.union_eff(le, se)))
                }
                Builtin::ListAppend => {
                    if args.len() != 2 {
                        return Err(at(*span, "append takes 2 arguments"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (et, ee) = self.infer_expr(&args[1])?;
                    let list_ty = match self.prune(lt.clone()) {
                        Type::List(t) => {
                            self.unify_at(*span, et, *t.clone())?;
                            Type::List(t)
                        }
                        Type::Var(_) => {
                            self.unify_at(*span, lt, Type::List(Box::new(et.clone())))?;
                            Type::List(Box::new(et))
                        }
                        other => {
                            return Err(at(*span, format!(
                                "append: expected List, got {other:?}"
                            )));
                        }
                    };
                    Ok((list_ty, self.union_eff(le, ee)))
                }
                Builtin::ListConcat => {
                    if args.len() != 2 {
                        return Err(at(*span, "concat takes 2 arguments"));
                    }
                    let (lt, le) = self.infer_expr(&args[0])?;
                    let (rt, re) = self.infer_expr(&args[1])?;
                    let lt = self.prune(lt);
                    let rt = self.prune(rt);
                    match (&lt, &rt) {
                        (Type::String, Type::String)
                        | (Type::String, Type::Var(_))
                        | (Type::Var(_), Type::String) => {
                            self.unify_at(*span, lt, Type::String)?;
                            self.unify_at(*span, rt, Type::String)?;
                            Ok((Type::String, self.union_eff(le, re)))
                        }
                        _ => {
                            let list_ty = match (lt.clone(), rt.clone()) {
                                (Type::List(a), Type::List(b)) => {
                                    self.unify_at(*span, *a.clone(), *b)?;
                                    Type::List(a)
                                }
                                (Type::List(a), Type::Var(_)) => {
                                    self.unify_at(*span, rt, Type::List(a.clone()))?;
                                    Type::List(a)
                                }
                                (Type::Var(_), Type::List(b)) => {
                                    self.unify_at(*span, lt, Type::List(b.clone()))?;
                                    Type::List(b)
                                }
                                (Type::Var(_), Type::Var(_)) => {
                                    let elem = self.fresh();
                                    let list = Type::List(Box::new(elem));
                                    self.unify_at(*span, lt, list.clone())?;
                                    self.unify_at(*span, rt, list.clone())?;
                                    list
                                }
                                (other, _) => {
                                    return Err(at(*span, format!(
                                        "concat: expected List or String, got {other:?}"
                                    )));
                                }
                            };
                            Ok((list_ty, self.union_eff(le, re)))
                        }
                    }
                }
                Builtin::Range | Builtin::RangeInclusive => {
                    if args.len() != 2 {
                        return Err(at(*span, "range takes 2 arguments"));
                    }
                    let mut eff = Effect::pure();
                    for a in args {
                        let (t, e) = self.infer_expr(a)?;
                        self.unify_at(*span, t, Type::Int)?;
                        eff = self.union_eff(eff, e);
                    }
                    Ok((Type::List(Box::new(Type::Int)), eff))
                }
                Builtin::Show => {
                    if args.len() != 1 {
                        return Err(at(*span, "show takes 1 argument"));
                    }
                    let (_, e) = self.infer_expr(&args[0])?;
                    Ok((Type::String, e))
                }
                Builtin::StrTrim | Builtin::StrToLower | Builtin::StrToUpper => {
                    if args.len() != 1 {
                        return Err(at(*span, format!(
                            "{name:?} takes 1 argument"
                        )));
                    }
                    let (st, se) = self.infer_expr(&args[0])?;
                    self.unify_at(*span, st, Type::String)?;
                    Ok((Type::String, se))
                }
                Builtin::StrSplit => {
                    if args.len() != 2 {
                        return Err(at(*span, "split takes 2 arguments"));
                    }
                    let (st, se) = self.infer_expr(&args[0])?;
                    let (ct, ce) = self.infer_expr(&args[1])?;
                    self.unify_at(*span, st, Type::String)?;
                    self.unify_at(*span, ct, Type::Char)?;
                    Ok((Type::List(Box::new(Type::String)), self.union_eff(se, ce)))
                }
                Builtin::StrSubstring => {
                    if args.len() != 3 {
                        return Err(at(*span, 
                            "substring takes 3 arguments (string, start, end)",
                        ));
                    }
                    let (st, se) = self.infer_expr(&args[0])?;
                    let (a, ae) = self.infer_expr(&args[1])?;
                    let (b, be) = self.infer_expr(&args[2])?;
                    self.unify_at(*span, st, Type::String)?;
                    self.unify_at(*span, a, Type::Int)?;
                    self.unify_at(*span, b, Type::Int)?;
                    Ok((Type::String, self.union3_eff(se, ae, be)))
                }
                Builtin::StrStartsWith | Builtin::StrEndsWith => {
                    if args.len() != 2 {
                        return Err(at(*span, 
                            "startsWith/endsWith takes 2 arguments",
                        ));
                    }
                    let (st, se) = self.infer_expr(&args[0])?;
                    let (pt, pe) = self.infer_expr(&args[1])?;
                    self.unify_at(*span, st, Type::String)?;
                    self.unify_at(*span, pt, Type::String)?;
                    Ok((Type::Bool, self.union_eff(se, pe)))
                }
                Builtin::ReadStdin => {
                    if !args.is_empty() {
                        return Err(at(*span, "readStdin takes 0 arguments"));
                    }
                    Ok((Type::String, Effect::io()))
                }
                Builtin::MatchFail => {
                    if !args.is_empty() {
                        return Err(at(*span, "match fail takes 0 arguments"));
                    }
                    // Diverges; fresh var unifies with any arm result type.
                    Ok((self.fresh(), Effect::pure()))
                }
                Builtin::Assert => {
                    if args.len() != 1 {
                        return Err(at(*span, "assert takes 1 argument"));
                    }
                    let (ct, ce) = self.infer_expr(&args[0])?;
                    self.unify_at(*span, ct, Type::Bool)?;
                    Ok((Type::Unit, ce))
                }
            },
            Expr::Binary {
                op, left, right, span,
            } => {
                let (lt, le) = self.infer_expr(left)?;
                let (rt, re) = self.infer_expr(right)?;
                let eff = self.union_eff(le, re);
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                        let lt = self.prune(lt);
                        let rt = self.prune(rt);
                        match (&lt, &rt) {
                            (Type::Float, _) | (_, Type::Float) => {
                                self.unify_at(*span, lt, Type::Float)?;
                                self.unify_at(*span, rt, Type::Float)?;
                                Ok((Type::Float, eff))
                            }
                            _ => {
                                self.unify_at(*span, lt, Type::Int)?;
                                self.unify_at(*span, rt, Type::Int)?;
                                Ok((Type::Int, eff))
                            }
                        }
                    }
                    BinOp::Eq | BinOp::Ne => {
                        self.unify_at(*span, lt.clone(), rt)?;
                        Ok((Type::Bool, eff))
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        // DESIGN Ord: Int/Float/Bool/String/Char until type classes land.
                        // Rejecting List/Map/ADT/… avoids pointer-bit compares at runtime.
                        self.unify_at(*span, lt.clone(), rt.clone())?;
                        let t = self.prune(lt);
                        match &t {
                            Type::Int
                            | Type::Float
                            | Type::Bool
                            | Type::String
                            | Type::Char
                            | Type::Var(_) => Ok((Type::Bool, eff)),
                            other => Err(at(
                                *span,
                                format!(
                                    "`<`/`<=`/`>`/`>=` need Ord (Int, Float, Bool, String, or Char), got {other}"
                                ),
                            )),
                        }
                    }
                    BinOp::And | BinOp::Or => {
                        self.unify_at(*span, lt, Type::Bool)?;
                        self.unify_at(*span, rt, Type::Bool)?;
                        Ok((Type::Bool, eff))
                    }
                }
            }
            Expr::Unary { op, expr, span } => {
                let (t, e) = self.infer_expr(expr)?;
                match op {
                    UnOp::Neg => {
                        let t = self.prune(t);
                        match t {
                            Type::Float => Ok((Type::Float, e)),
                            _ => {
                                self.unify_at(*span, t, Type::Int)?;
                                Ok((Type::Int, e))
                            }
                        }
                    }
                    UnOp::Not => {
                        self.unify_at(*span, t, Type::Bool)?;
                        Ok((Type::Bool, e))
                    }
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                span,
            } => {
                let (ct, ce) = self.infer_expr(cond)?;
                self.unify_at(*span, ct, Type::Bool)?;
                let (tt, te) = self.infer_expr(then_branch)?;
                let (et, ee) = self.infer_expr(else_branch)?;
                let joined = self.join_types(tt, et, *span)?;
                Ok((joined, self.union3_eff(ce, te, ee)))
            }
            Expr::Loop {
                cond,
                body,
                step,
                span,
            } => {
                let (ct, ce) = self.infer_expr(cond)?;
                self.unify_at(*span, ct, Type::Bool)?;
                let (_, be) = self.infer_expr(body)?;
                let se = if let Some(s) = step {
                    self.infer_expr(s)?.1
                } else {
                    Effect::pure()
                };
                Ok((Type::Unit, self.union3_eff(ce, be, se)))
            }
            Expr::Break(_) | Expr::Continue(_) => Ok((Type::Unit, Effect::pure())),
            Expr::AdtNew {
                adt_name,
                variant,
                args,
                ..
            } => {
                let mut eff = Effect::pure();
                let mut arg_tys = vec![];
                for a in args {
                    let (t, e) = self.infer_expr(a)?;
                    arg_tys.push(t);
                    eff = self.union_eff(eff, e);
                }
                if adt_name == "__Tuple" {
                    return Ok((Type::Tuple(arg_tys), eff));
                }
                // Result[T, E]: Ok fills T (E fresh); Err fills E (T fresh).
                let params = if adt_name == "Result" {
                    match (variant.as_str(), arg_tys.as_slice()) {
                        ("Ok", [t]) => vec![t.clone(), self.fresh()],
                        ("Err", [e]) => vec![self.fresh(), e.clone()],
                        _ if arg_tys.is_empty() => vec![self.fresh(), self.fresh()],
                        _ => arg_tys,
                    }
                } else if arg_tys.is_empty() {
                    // Unit ctors: Option[α] with fresh α.
                    vec![self.fresh()]
                } else {
                    // Product / unary sum: field / payload types as params.
                    arg_tys
                };
                Ok((
                    Type::Adt {
                        name: adt_name.clone(),
                        params,
                    },
                    eff,
                ))
            }
            Expr::Seq { stmts, .. } => {
                let mut eff = Effect::pure();
                let mut last = Type::Unit;
                for s in stmts {
                    let (t, e) = self.infer_expr(s)?;
                    last = t;
                    eff = self.union_eff(eff, e);
                }
                Ok((last, eff))
            }
        }
    }

    fn infer_fun(&mut self, fun: &Fun) -> Result<(Type, Effect), TypeError> {
        self.push();
        let mut pts = vec![];
        for p in &fun.params {
            let tv = self.fresh();
            pts.push(tv.clone());
            self.bind(p.clone(), tv);
        }
        let (rt, re) = self.infer_expr(&fun.body)?;
        // main is always an effect root
        let re = if fun.is_main {
            self.union_eff(re, Effect::io())
        } else {
            re
        };
        self.pop();
        let ty = Type::Fun(pts, Box::new(rt), re);
        Ok((ty, re))
    }
}

fn occurs(v: u32, ty: &Type) -> bool {
    match ty {
        Type::Var(u) => *u == v,
        Type::Fun(ps, r, _) => ps.iter().any(|p| occurs(v, p)) || occurs(v, r),
        Type::List(t) => occurs(v, t),
        Type::Set(t) => occurs(v, t),
        Type::Map(k, t) => occurs(v, k) || occurs(v, t),
        Type::Adt { params, .. } => params.iter().any(|p| occurs(v, p)),
        Type::Tuple(ts) => ts.iter().any(|p| occurs(v, p)),
        _ => false,
    }
}

fn parse_foreign_type(name: &str) -> Result<Type, TypeError> {
    match name {
        "Int" => Ok(Type::Int),
        "Bool" => Ok(Type::Bool),
        "Float" => Ok(Type::Float),
        "Unit" => Ok(Type::Unit),
        "String" => Ok(Type::String),
        other => Err(TypeError::Message(format!(
            "unsupported foreign type `{other}` (supported: Int, Bool, Float, Unit, String)"
        ))),
    }
}

/// Options for module inference (FFI trust, etc.).
#[derive(Debug, Clone, Default)]
pub struct InferOptions {
    /// Honor `foreign "C" pure` as [`Effect::Pure`]. Without this, `pure` is rejected
    /// (FFI purity is not verified; default foreign effect is IO).
    pub trust_foreign_pure: bool,
}

pub fn infer_module(module: &Module) -> Result<TypedModule, TypeError> {
    infer_module_with_visibility(module, NameVisibility::default())
}

pub fn infer_module_with_visibility(
    module: &Module,
    vis: NameVisibility,
) -> Result<TypedModule, TypeError> {
    infer_module_with_options(module, vis, InferOptions::default())
}

pub fn infer_module_with_options(
    module: &Module,
    vis: NameVisibility,
    opts: InferOptions,
) -> Result<TypedModule, TypeError> {
    let mut inf = Infer::new(vis);
    let mut fun_types = HashMap::new();
    let mut main_effect = Effect::pure();

    // First pass: bind function names with fresh types for recursion
    for item in &module.items {
        if let Item::Fun(f) = item {
            let tv = inf.fresh();
            inf.bind(f.name.clone(), tv);
        }
    }

    for item in &module.items {
        match item {
            Item::Fun(f) => {
                inf.current_file = expr_span(&f.body).file;
                let (ty, eff) = if let Some((ptys, ret)) = &f.foreign_sig {
                    let ps: Result<Vec<_>, _> = ptys.iter().map(|t| parse_foreign_type(t)).collect();
                    let ps = ps?;
                    let r = parse_foreign_type(ret)?;
                    // Default: foreign is IO. `pure` is an honor-system claim and
                    // requires `--trust-foreign-pure` / `package.trust_foreign_pure`.
                    // Opts still never CSE/memo/inline externals (`lumia_opt`).
                    let eff = if f.foreign_pure {
                        if !opts.trust_foreign_pure {
                            return Err(at(
                                expr_span(&f.body),
                                "`foreign \"C\" pure` requires `--trust-foreign-pure` \
                                 (or `package.trust_foreign_pure = true`); FFI purity is \
                                 not verified — omit `pure` to type the import as IO",
                            ));
                        }
                        Effect::pure()
                    } else {
                        Effect::io()
                    };
                    (Type::Fun(ps, Box::new(r), eff), eff)
                } else {
                    inf.infer_fun(f)?
                };
                if let Some(existing) = inf.lookup(&f.name) {
                    inf.unify(existing, ty.clone())?;
                }
                let ty = inf.prune(ty);
                let scheme = inf.generalize(ty.clone());
                inf.rebind_scheme(&f.name, scheme)?;
                fun_types.insert(f.name.clone(), ty);
                // Decl span: use body span as stand-in for foreign/unit; funs lack item span in HIR.
                inf.decls.insert(f.name.clone(), expr_span(&f.body));
                if f.is_main {
                    main_effect = eff;
                    if !eff.has_io() {
                        main_effect = Effect::io();
                    }
                }
            }
            Item::Val { name, body } => {
                inf.current_file = expr_span(body).file;
                let (ty, eff) = inf.infer_expr(body)?;
                if inf.prune_eff(eff).has_io() {
                    return Err(at(
                        expr_span(body),
                        format!(
                            "module-level `{name}` initializer must be pure (got IO effect)"
                        ),
                    ));
                }
                let ty = inf.prune(ty);
                let scheme = inf.generalize(ty.clone());
                inf.bind_scheme(name.clone(), scheme, false);
                inf.decls.insert(name.clone(), expr_span(body));
                // Zero-arg getter used by Core lowering / codegen GC rooting.
                fun_types.insert(
                    format!("__val_{name}"),
                    Type::Fun(vec![], Box::new(ty), Effect::pure()),
                );
            }
        }
    }

    // Resolve open effect vars (unconstrained → Pure; Io bound via later call sites).
    for ty in fun_types.values_mut() {
        *ty = inf.zonk_type(ty.clone());
    }
    main_effect = inf.zonk_eff(main_effect);

    let type_at_raw = std::mem::take(&mut inf.type_at);
    let type_at: Vec<_> = type_at_raw
        .into_iter()
        .map(|(sp, t)| (sp, inf.zonk_type(t)))
        .collect();
    let decls = std::mem::take(&mut inf.decls);
    Ok(TypedModule {
        module: module.clone(),
        fun_types,
        main_effect,
        type_at,
        decls,
    })
}

/// Reject calling effectful functions from pure contexts (simplified whole-program check).
pub fn check_effect_boundaries(typed: &TypedModule) -> Result<(), TypeError> {
    for item in &typed.module.items {
        if let Item::Fun(f) = item {
            let fun_ty = typed.fun_types.get(&f.name);
            let fun_is_effectful = match fun_ty {
                Some(Type::Fun(_, _, e)) => e.has_io() || f.is_main,
                _ => f.is_main,
            };
            // If inference claims pure, body must not contain any effect
            if !fun_is_effectful {
                assert_no_effects_in_pure(&f.body, &typed.fun_types)?;
            }
            check_expr_effects(&f.body, fun_is_effectful, &typed.fun_types)?;
        }
    }
    Ok(())
}

fn assert_no_effects_in_pure(
    expr: &Expr,
    fun_types: &HashMap<String, Type>,
) -> Result<(), TypeError> {
    match expr {
        Expr::BuiltinCall { name, args, span } => {
            match name {
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr | Builtin::ReadStdin => {
                    return Err(at(*span, 
                        "effectful call not allowed in pure function",
                    ));
                }
                _ => {
                    for a in args {
                        assert_no_effects_in_pure(a, fun_types)?;
                    }
                    Ok(())
                }
            }
        }
        Expr::Call { callee, args, span } => {
            if let Expr::Var(name, _) = callee.as_ref() {
                if let Some(Type::Fun(_, _, e)) = fun_types.get(name) {
                    if e.has_io() {
                        return Err(at(*span, format!(
                            "cannot call effectful `{name}` from pure function"
                        )));
                    }
                }
            }
            assert_no_effects_in_pure(callee, fun_types)?;
            for a in args {
                assert_no_effects_in_pure(a, fun_types)?;
            }
            Ok(())
        }
        Expr::Let { value, body, .. } => {
            assert_no_effects_in_pure(value, fun_types)?;
            assert_no_effects_in_pure(body, fun_types)
        }
        Expr::Assign { value, .. } => assert_no_effects_in_pure(value, fun_types),
        Expr::Lambda { body, .. } => {
            let _ = body;
            Ok(())
        }
        Expr::Binary { left, right, .. } => {
            assert_no_effects_in_pure(left, fun_types)?;
            assert_no_effects_in_pure(right, fun_types)
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                assert_no_effects_in_pure(s, fun_types)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } => assert_no_effects_in_pure(expr, fun_types),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            assert_no_effects_in_pure(cond, fun_types)?;
            assert_no_effects_in_pure(then_branch, fun_types)?;
            assert_no_effects_in_pure(else_branch, fun_types)
        }
        Expr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            assert_no_effects_in_pure(cond, fun_types)?;
            assert_no_effects_in_pure(body, fun_types)?;
            if let Some(s) = step {
                assert_no_effects_in_pure(s, fun_types)?;
            }
            Ok(())
        }
        Expr::Break(_) | Expr::Continue(_) => Ok(()),
        Expr::AdtNew { args, .. } => {
            for a in args {
                assert_no_effects_in_pure(a, fun_types)?;
            }
            Ok(())
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..) => Ok(()),
    }
}

fn check_expr_effects(
    expr: &Expr,
    in_effect_ctx: bool,
    fun_types: &HashMap<String, Type>,
) -> Result<(), TypeError> {
    match expr {
        Expr::BuiltinCall { name, args, span } => {
            match name {
                Builtin::Println | Builtin::PrintlnInt | Builtin::PrintlnStr | Builtin::ReadStdin => {
                    if !in_effect_ctx {
                        return Err(at(*span, 
                            "effectful call not allowed in pure context",
                        ));
                    }
                }
                _ => {}
            }
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Call { callee, args, span } => {
            if let Expr::Var(name, _) = callee.as_ref() {
                if let Some(Type::Fun(_, _, e)) = fun_types.get(name) {
                    if e.has_io() && !in_effect_ctx {
                        return Err(at(*span, format!(
                            "cannot call effectful `{name}` from pure context"
                        )));
                    }
                }
            }
            check_expr_effects(callee, in_effect_ctx, fun_types)?;
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Let { value, body, .. } => {
            check_expr_effects(value, in_effect_ctx, fun_types)?;
            check_expr_effects(body, in_effect_ctx, fun_types)
        }
        Expr::Assign { value, .. } => check_expr_effects(value, in_effect_ctx, fun_types),
        Expr::Lambda { body, .. } => {
            // Lambda bodies are their own effect context: effectful bodies are OK
            // (the Fun type carries ε); check under an effectful context.
            check_expr_effects(body, true, fun_types)
        }
        Expr::Binary { left, right, .. } => {
            check_expr_effects(left, in_effect_ctx, fun_types)?;
            check_expr_effects(right, in_effect_ctx, fun_types)
        }
        Expr::Seq { stmts, .. } => {
            for s in stmts {
                check_expr_effects(s, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Unary { expr, .. } => check_expr_effects(expr, in_effect_ctx, fun_types),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            check_expr_effects(cond, in_effect_ctx, fun_types)?;
            check_expr_effects(then_branch, in_effect_ctx, fun_types)?;
            check_expr_effects(else_branch, in_effect_ctx, fun_types)
        }
        Expr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            check_expr_effects(cond, in_effect_ctx, fun_types)?;
            check_expr_effects(body, in_effect_ctx, fun_types)?;
            if let Some(s) = step {
                check_expr_effects(s, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Break(_) | Expr::Continue(_) => Ok(()),
        Expr::AdtNew { args, .. } => {
            for a in args {
                check_expr_effects(a, in_effect_ctx, fun_types)?;
            }
            Ok(())
        }
        Expr::Int(..)
        | Expr::Float(..)
        | Expr::Bool(..)
        | Expr::String(..)
        | Expr::Char(..)
        | Expr::Unit(..)
        | Expr::Var(..) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumia_hir::lower_module;
    use lumia_syntax::parse_module;

    #[test]
    fn infer_hello() {
        let src = r#"
module Hello
import std.io.{println}
val main = {
    println(42)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
        assert!(typed.main_effect.has_io());
    }

    #[test]
    fn ord_rejects_list_compare() {
        let src = r#"
module BadOrd
val main = {
    listOf(1) < listOf(2)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let err = infer_module(&hir).expect_err("List is not Ord");
        assert!(
            err.message().contains("Ord") || err.message().contains("List"),
            "unexpected: {}",
            err.message()
        );
    }

    #[test]
    fn let_polymorphism_identity() {
        let src = r#"
module LetPoly
import std.io.{println}
val main = {
    val id = { x -> x }
    println(id(1))
    println(id("hi"))
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("let-poly id");
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn reject_println_inside_pure_lambda_used_as_value() {
        let src = r#"
module Bad
import std.io.{println}
val compute() = {
    println(1)
    0
}
val main = {
    compute()
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
        assert!(matches!(
            typed.fun_types.get("compute"),
            Some(Type::Fun(_, _, Effect::Io))
        ));
    }

    #[test]
    fn module_val_rejects_io() {
        let src = r#"
module Bad
import std.io.{println}
val xs = println(1)
val main = {
    0
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        assert!(infer_module(&hir).is_err());
    }

    #[test]
    fn map_of_empty() {
        let src = r#"
module M
val m = mapOf()
val main = {
    0
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn list_of_infers() {
        let src = r#"
module L
val xs = listOf(1, 2, 3)
val main = {
    0
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn if_and_add() {
        let src = r#"
module I
import std.io.{println}
val main = {
    val x = if true { 1 } else { 2 }
    println(x + 40)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).unwrap();
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn hof_picks_up_callback_io() {
        let src = r#"
module Hof
import std.io.{println}
val apply(f, x) = f(x)
val boom(x) = {
    println(1)
    x
}
val main = {
    apply(boom, 42)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        check_effect_boundaries(&typed).unwrap();
        assert!(matches!(
            typed.fun_types.get("apply"),
            Some(Type::Fun(_, _, Effect::Io))
        ));
        assert!(matches!(
            typed.fun_types.get("boom"),
            Some(Type::Fun(_, _, Effect::Io))
        ));
    }

    #[test]
    fn hof_stays_pure_with_pure_callback() {
        let src = r#"
module HofPure
val apply(f, x) = f(x)
val id(x) = x
val main = {
    apply(id, 42)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        check_effect_boundaries(&typed).unwrap();
        assert!(matches!(
            typed.fun_types.get("apply"),
            Some(Type::Fun(_, _, Effect::Pure))
        ));
    }

    #[test]
    fn match_int_arms() {
        let src = r#"
module MatchDemo
import std.io.{println}
val main = {
    val n = 1
    val s = n match {
        0 -> 10
        1 -> 20
        _ -> 30
    }
    println(s)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn parallel_map_rejects_io_callback() {
        use lumia_hir::set_parallel_map;
        let src = r#"
module ParIo
import std.io.{println}
val boom(x) = {
    println(x + 0)
    x + 1
}
val main = {
    listOf(1, 2, 3).map(boom)
}
"#;
        let ast = parse_module(src).unwrap();
        set_parallel_map(true);
        let hir = lower_module(&ast).expect("lower");
        set_parallel_map(false);
        let err = infer_module(&hir).expect_err("IO callback must fail under --parallel");
        let msg = err.to_string();
        assert!(
            msg.contains("parallel map") && msg.contains("pure"),
            "expected parallel purity error, got {msg}"
        );
    }

    /// `if` arms joining Pure/Io function values must lub to Io on the caller.
    #[test]
    fn if_branches_io_vs_pure_fun_marks_caller_or_rejects() {
        let src = r#"
module Hole
import std.io.{println}
val id(x) = x
val boom(x) = {
    println(x + 0)
    x
}
val sneak(c, x) = {
    val f = if c { id } else { boom }
    f(x)
}
val main = {
    sneak(false, 1)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        assert!(
            matches!(
                typed.fun_types.get("sneak"),
                Some(Type::Fun(_, _, Effect::Io))
            ),
            "if-branch Fun lub must mark sneak Io; got {:?}",
            typed.fun_types.get("sneak")
        );
        check_effect_boundaries(&typed).unwrap();
    }

    /// Assigning an Io lambda into a `var` previously holding a Pure lambda widens ε.
    #[test]
    fn assign_io_fun_into_pure_var_marks_caller_or_rejects() {
        let src = r#"
module Hole
import std.io.{println}
val sneak(x) = {
    var f = { y -> y }
    f = { y ->
        println(y + 0)
        y
    }
    f(x)
}
val main = {
    sneak(1)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        assert!(
            matches!(
                typed.fun_types.get("sneak"),
                Some(Type::Fun(_, _, Effect::Io))
            ),
            "assign Fun lub must mark sneak Io; got {:?}",
            typed.fun_types.get("sneak")
        );
        check_effect_boundaries(&typed).unwrap();
    }

    /// Two open callback effects in one body must not drop the second Var.
    #[test]
    fn hof_two_callbacks_union_preserves_io() {
        let src = r#"
module Both
import std.io.{println}
val both(f, g, x) = {
    f(x)
    g(x)
}
val id(x) = x
val boom(x) = {
    println(x + 0)
    x
}
val sneak(x) = both(id, boom, x)
val main = {
    sneak(1)
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        assert!(
            matches!(
                typed.fun_types.get("both"),
                Some(Type::Fun(_, _, Effect::Io))
            ),
            "both must be Io when either callback is Io; got {:?}",
            typed.fun_types.get("both")
        );
        assert!(
            matches!(
                typed.fun_types.get("sneak"),
                Some(Type::Fun(_, _, Effect::Io))
            ),
            "sneak must be Io; got {:?}",
            typed.fun_types.get("sneak")
        );
        check_effect_boundaries(&typed).unwrap();
    }

    #[test]
    fn foreign_pure_requires_trust() {
        let src = r#"
module F
foreign "C" pure fn llabs(x: Int) -> Int
val main = { llabs(1) }
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let err = infer_module(&hir).expect_err("pure without trust");
        assert!(
            err.message().contains("trust-foreign-pure"),
            "got {}",
            err.message()
        );
    }

    #[test]
    fn foreign_pure_trusted_is_pure() {
        let src = r#"
module F
foreign "C" pure fn llabs(x: Int) -> Int
val main = { llabs(1) }
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module_with_options(
            &hir,
            NameVisibility::default(),
            InferOptions {
                trust_foreign_pure: true,
            },
        )
        .expect("trusted pure");
        assert!(matches!(
            typed.fun_types.get("llabs"),
            Some(Type::Fun(_, _, Effect::Pure))
        ));
    }

    #[test]
    fn foreign_without_pure_is_io() {
        let src = r#"
module F
foreign "C" fn getenv(s: String) -> String
val main = { getenv("PATH") }
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("lower");
        let typed = infer_module(&hir).expect("infer");
        assert!(matches!(
            typed.fun_types.get("getenv"),
            Some(Type::Fun(_, _, Effect::Io))
        ));
    }
}

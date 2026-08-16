//! Unification, generalization, and effect joining.

use super::Infer;
use crate::types::{at, locate, Effect, Scheme, Type, TypeError};
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};

impl Infer {
    pub(crate) fn free_ty_vars(&mut self, ty: Type) -> HashSet<u32> {
        let ty = self.prune(ty);
        let mut acc = HashSet::default();
        self.collect_ty_vars(&ty, &mut acc);
        acc
    }

    pub(crate) fn collect_ty_vars(&mut self, ty: &Type, acc: &mut HashSet<u32>) {
        self.collect_ty_vars_rec(ty, acc, &mut HashSet::default());
    }

    fn collect_ty_vars_rec(
        &mut self,
        ty: &Type,
        acc: &mut HashSet<u32>,
        seen: &mut HashSet<u32>,
    ) {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.uni.subst.get(v).cloned() {
                    if !seen.insert(*v) {
                        return;
                    }
                    // Walk the binding directly (do not prune — cyclic prune
                    // returns `Var(v)` and would stop collection too early).
                    self.collect_ty_vars_rec(&t, acc, seen);
                    seen.remove(v);
                } else {
                    acc.insert(*v);
                }
            }
            Type::Fun(ps, r, _) => {
                for p in ps {
                    self.collect_ty_vars_rec(p, acc, seen);
                }
                self.collect_ty_vars_rec(r, acc, seen);
            }
            Type::List(t) | Type::Set(t) | Type::Task(t) | Type::Channel(t) => {
                self.collect_ty_vars_rec(t, acc, seen);
            }
            Type::Map(k, v) => {
                self.collect_ty_vars_rec(k, acc, seen);
                self.collect_ty_vars_rec(v, acc, seen);
            }
            Type::Adt { params, .. } | Type::Tuple(params) | Type::TuplePrefix(params) => {
                for p in params {
                    self.collect_ty_vars_rec(p, acc, seen);
                }
            }
            _ => {}
        }
    }

    /// Vars under `Channel[…]` must stay monomorphic (value restriction).
    /// Otherwise `val ch = channel(1)` generalizes to `∀α. Channel[α]`, so
    /// `send(Some(x))` and `recv() alt …` see different α and alt rejects Var.
    pub(crate) fn channel_escaping_ty_vars(&mut self, ty: Type) -> HashSet<u32> {
        let ty = self.prune(ty);
        let mut acc = HashSet::default();
        self.collect_channel_escaping_ty_vars(&ty, &mut acc, &mut HashSet::default());
        acc
    }

    fn collect_channel_escaping_ty_vars(
        &mut self,
        ty: &Type,
        acc: &mut HashSet<u32>,
        seen: &mut HashSet<u32>,
    ) {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.uni.subst.get(v).cloned() {
                    if !seen.insert(*v) {
                        return;
                    }
                    self.collect_channel_escaping_ty_vars(&t, acc, seen);
                    seen.remove(v);
                }
            }
            Type::Channel(t) => self.collect_ty_vars(t, acc),
            Type::Fun(ps, r, _) => {
                for p in ps {
                    self.collect_channel_escaping_ty_vars(p, acc, seen);
                }
                self.collect_channel_escaping_ty_vars(r, acc, seen);
            }
            Type::List(t) | Type::Set(t) | Type::Task(t) => {
                self.collect_channel_escaping_ty_vars(t, acc, seen)
            }
            Type::Map(k, v) => {
                self.collect_channel_escaping_ty_vars(k, acc, seen);
                self.collect_channel_escaping_ty_vars(v, acc, seen);
            }
            Type::Adt { params, .. } => {
                for p in params {
                    self.collect_channel_escaping_ty_vars(p, acc, seen);
                }
            }
            Type::Tuple(ts) | Type::TuplePrefix(ts) => {
                for t in ts {
                    self.collect_channel_escaping_ty_vars(t, acc, seen);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn env_free_ty_vars(&mut self) -> HashSet<u32> {
        let schemes: Vec<Scheme> = self
            .scopes
            .env
            .iter()
            .flat_map(|scope| scope.values().cloned())
            .collect();
        let mut acc = HashSet::default();
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

    pub(crate) fn generalize(&mut self, ty: Type) -> Scheme {
        let ty = self.prune(ty);
        let env_fvs = self.env_free_ty_vars();
        let channel_fvs = self.channel_escaping_ty_vars(ty.clone());
        let mut vars: Vec<u32> = self
            .free_ty_vars(ty.clone())
            .into_iter()
            .filter(|v| !env_fvs.contains(v) && !channel_fvs.contains(v))
            .collect();
        vars.sort_unstable();
        // Leave effect vars free (not quantified): module-level HOF use can still
        // refine `apply`/`both` to Io via shared `Effect::Var`, then zonk into
        // `fun_types`. Quantifying them would freeze Pure before call sites run.
        let mut num_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.num_vars.contains(v))
            .collect();
        num_vars.sort_unstable();
        let mut ord_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.ord_vars.contains(v))
            .collect();
        ord_vars.sort_unstable();
        let mut eq_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.eq_vars.contains(v))
            .collect();
        eq_vars.sort_unstable();
        let mut len_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.len_vars.contains(v))
            .collect();
        len_vars.sort_unstable();
        let mut concat_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.concat_vars.contains(v))
            .collect();
        concat_vars.sort_unstable();
        let mut contains_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.contains_vars.contains(v))
            .collect();
        contains_vars.sort_unstable();
        let mut set_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.set_vars.contains(v))
            .collect();
        set_vars.sort_unstable();
        let mut elems_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.elems_vars.contains(v))
            .collect();
        elems_vars.sort_unstable();
        let mut take_vars: Vec<u32> = vars
            .iter()
            .copied()
            .filter(|v| self.uni.take_vars.contains(v))
            .collect();
        take_vars.sort_unstable();
        let mut trait_preds: Vec<(u32, String, String)> = Vec::new();
        for &v in &vars {
            if let Some(preds) = self.traits.trait_vars.get(&v) {
                for (tr, method) in preds {
                    trait_preds.push((v, tr.clone(), method.clone()));
                }
            }
        }
        trait_preds.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        Scheme {
            vars,
            ty,
            num_vars,
            ord_vars,
            eq_vars,
            len_vars,
            concat_vars,
            contains_vars,
            set_vars,
            elems_vars,
            take_vars,
            trait_preds,
        }
    }

    pub(crate) fn instantiate(&mut self, scheme: &Scheme) -> Type {
        let ty_map: HashMap<u32, Type> = scheme.vars.iter().map(|&v| (v, self.fresh())).collect();
        for &old in &scheme.num_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.num_vars.insert(*n);
            }
        }
        for &old in &scheme.ord_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.ord_vars.insert(*n);
            }
        }
        for &old in &scheme.eq_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.eq_vars.insert(*n);
            }
        }
        for &old in &scheme.len_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.len_vars.insert(*n);
            }
        }
        for &old in &scheme.concat_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.concat_vars.insert(*n);
            }
        }
        for &old in &scheme.contains_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.contains_vars.insert(*n);
            }
        }
        for &old in &scheme.set_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.set_vars.insert(*n);
            }
        }
        for &old in &scheme.elems_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.elems_vars.insert(*n);
            }
        }
        for &old in &scheme.take_vars {
            if let Some(Type::Var(n)) = ty_map.get(&old) {
                self.uni.take_vars.insert(*n);
            }
        }
        for (old, tr, method) in &scheme.trait_preds {
            if let Some(Type::Var(n)) = ty_map.get(old) {
                self.traits
                    .trait_vars
                    .entry(*n)
                    .or_default()
                    .push((tr.clone(), method.clone()));
            }
        }
        let eff_map: HashMap<u32, Effect> = HashMap::default();
        self.apply_scheme_subst(&scheme.ty, &ty_map, &eff_map)
    }

    pub(crate) fn apply_scheme_subst(
        &mut self,
        ty: &Type,
        ty_map: &HashMap<u32, Type>,
        eff_map: &HashMap<u32, Effect>,
    ) -> Type {
        // Do not prune `Var` roots: params stay as `Var(v)` so later constraints
        // (TuplePrefix extension, Num/Ord binds) can rebind `v`. Callers prune
        // when they need the concrete shape.
        match ty {
            Type::Var(v) => ty_map.get(v).cloned().unwrap_or(Type::Var(*v)),
            Type::Fun(ps, r, e) => {
                let e = match self.prune_eff(*e) {
                    Effect::Var(v) => eff_map.get(&v).copied().unwrap_or(Effect::Var(v)),
                    other => other,
                };
                Type::Fun(
                    ps.iter()
                        .map(|p| self.apply_scheme_subst(p, ty_map, eff_map))
                        .collect(),
                    Box::new(self.apply_scheme_subst(r, ty_map, eff_map)),
                    e,
                )
            }
            Type::List(t) => Type::List(Box::new(self.apply_scheme_subst(t, ty_map, eff_map))),
            Type::Set(t) => Type::Set(Box::new(self.apply_scheme_subst(t, ty_map, eff_map))),
            Type::Task(t) => Type::Task(Box::new(self.apply_scheme_subst(t, ty_map, eff_map))),
            Type::Channel(t) => {
                Type::Channel(Box::new(self.apply_scheme_subst(t, ty_map, eff_map)))
            }
            Type::Map(k, v) => Type::Map(
                Box::new(self.apply_scheme_subst(k, ty_map, eff_map)),
                Box::new(self.apply_scheme_subst(v, ty_map, eff_map)),
            ),
            Type::Adt { name, params } => Type::Adt {
                name: name.clone(),
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
            Type::TuplePrefix(ts) => Type::TuplePrefix(
                ts.iter()
                    .map(|t| self.apply_scheme_subst(t, ty_map, eff_map))
                    .collect(),
            ),
            Type::Int => Type::Int,
            Type::Float => Type::Float,
            Type::Bool => Type::Bool,
            Type::String => Type::String,
            Type::Char => Type::Char,
            Type::Unit => Type::Unit,
        }
    }

    pub(crate) fn prune(&mut self, ty: Type) -> Type {
        self.prune_rec(ty, &mut HashSet::default())
    }

    fn prune_rec(&mut self, ty: Type, seen: &mut HashSet<u32>) -> Type {
        match ty {
            Type::Var(v) => {
                if let Some(t) = self.uni.subst.get(&v).cloned() {
                    if !seen.insert(v) {
                        // Equi-recursive Adt binding: stop expanding the cycle.
                        return Type::Var(v);
                    }
                    let t = self.prune_rec(t, seen);
                    seen.remove(&v);
                    if occurs(v, &t) {
                        // Keep the μ-binder as `Var(v)`. Returning the unfolded
                        // `Adt[…, Var(v), …]` here would re-expand on every prune
                        // of a parent Adt and blow the stack (`Expr { Lit Add }`).
                        Type::Var(v)
                    } else {
                        self.uni.subst.insert(v, t.clone());
                        t
                    }
                } else {
                    Type::Var(v)
                }
            }
            Type::Fun(ps, r, e) => Type::Fun(
                ps.into_iter()
                    .map(|p| self.prune_rec(p, seen))
                    .collect(),
                Box::new(self.prune_rec(*r, seen)),
                self.prune_eff(e),
            ),
            Type::List(t) => Type::List(Box::new(self.prune_rec(*t, seen))),
            Type::Map(k, v) => Type::Map(
                Box::new(self.prune_rec(*k, seen)),
                Box::new(self.prune_rec(*v, seen)),
            ),
            Type::Set(t) => Type::Set(Box::new(self.prune_rec(*t, seen))),
            Type::Task(t) => Type::Task(Box::new(self.prune_rec(*t, seen))),
            Type::Channel(t) => Type::Channel(Box::new(self.prune_rec(*t, seen))),
            Type::Adt { name, params } => Type::Adt {
                name,
                params: params
                    .into_iter()
                    .map(|p| self.prune_rec(p, seen))
                    .collect(),
            },
            Type::Tuple(ts) => {
                Type::Tuple(ts.into_iter().map(|t| self.prune_rec(t, seen)).collect())
            }
            Type::TuplePrefix(ts) => {
                Type::TuplePrefix(ts.into_iter().map(|t| self.prune_rec(t, seen)).collect())
            }
            other => other,
        }
    }

    pub(crate) fn prune_eff(&mut self, e: Effect) -> Effect {
        match e {
            Effect::Var(v) => {
                if let Some(e2) = self.uni.eff_subst.get(&v).cloned() {
                    let e2 = self.prune_eff(e2);
                    self.uni.eff_subst.insert(v, e2);
                    e2
                } else {
                    Effect::Var(v)
                }
            }
            other => other,
        }
    }

    /// Unbound effect vars become Pure (generalize as pure when unconstrained).
    pub(crate) fn zonk_eff(&mut self, e: Effect) -> Effect {
        match self.prune_eff(e) {
            Effect::Var(_) => Effect::Pure,
            other => other,
        }
    }

    pub(crate) fn zonk_type(&mut self, ty: Type) -> Type {
        self.zonk_type_rec(ty, &mut HashSet::default())
    }

    fn zonk_type_rec(&mut self, ty: Type, seen: &mut HashSet<u32>) -> Type {
        if let Type::Var(v) = ty {
            if let Some(t) = self.uni.subst.get(&v).cloned() {
                if !seen.insert(v) {
                    // Cycle: leave a named knot; parent Adt arm rewrites these
                    // into a finite `Adt { params: [] }` stub below.
                    return Type::Var(v);
                }
                let t = self.zonk_type_rec(t, seen);
                seen.remove(&v);
                return t;
            }
            return Type::Var(v);
        }
        match ty {
            Type::Fun(ps, r, e) => Type::Fun(
                ps.into_iter()
                    .map(|p| self.zonk_type_rec(p, seen))
                    .collect(),
                Box::new(self.zonk_type_rec(*r, seen)),
                self.zonk_eff(e),
            ),
            Type::List(t) => Type::List(Box::new(self.zonk_type_rec(*t, seen))),
            Type::Map(k, v) => Type::Map(
                Box::new(self.zonk_type_rec(*k, seen)),
                Box::new(self.zonk_type_rec(*v, seen)),
            ),
            Type::Set(t) => Type::Set(Box::new(self.zonk_type_rec(*t, seen))),
            Type::Task(t) => Type::Task(Box::new(self.zonk_type_rec(*t, seen))),
            Type::Channel(t) => Type::Channel(Box::new(self.zonk_type_rec(*t, seen))),
            Type::Adt { name, params } => Type::Adt {
                name: name.clone(),
                params: params
                    .into_iter()
                    .map(|p| match p {
                        // Fold equi-recursive spines to a finite stub so schemes
                        // do not export dangling cycle vars after subst drops.
                        Type::Var(u) if seen.contains(&u) => Type::Adt {
                            name: name.clone(),
                            params: vec![],
                        },
                        other => self.zonk_type_rec(other, seen),
                    })
                    .collect(),
            },
            Type::Tuple(ts) => Type::Tuple(
                ts.into_iter()
                    .map(|t| self.zonk_type_rec(t, seen))
                    .collect(),
            ),
            Type::TuplePrefix(ts) => Type::TuplePrefix(
                ts.into_iter()
                    .map(|t| self.zonk_type_rec(t, seen))
                    .collect(),
            ),
            Type::Var(_) => unreachable!("handled above"),
            other => other,
        }
    }

    /// Least upper bound of effects, linking distinct open vars so either becoming
    /// `Io` zonks both (needed for `f(x); g(x)` HOF bodies).
    pub(crate) fn union_eff(&mut self, a: Effect, b: Effect) -> Effect {
        let a = self.prune_eff(a);
        let b = self.prune_eff(b);
        match (a, b) {
            (Effect::Io, _) | (_, Effect::Io) => Effect::Io,
            (Effect::Pure, Effect::Pure) => Effect::Pure,
            (Effect::Var(v), Effect::Pure) | (Effect::Pure, Effect::Var(v)) => Effect::Var(v),
            (Effect::Var(a), Effect::Var(b)) => {
                if a != b {
                    self.uni.eff_subst.insert(a, Effect::Var(b));
                }
                Effect::Var(b)
            }
        }
    }

    pub(crate) fn union3_eff(&mut self, a: Effect, b: Effect, c: Effect) -> Effect {
        let ab = self.union_eff(a, b);
        self.union_eff(ab, c)
    }

    /// Unify effects for equality. `Pure` and `Io` are distinct (do **not** unify).
    /// Open `Var` stays flexible when matched with `Pure` so a later `Io` use can
    /// still instantiate it (HOF effect polymorphism); matching `Io` binds the var.
    pub(crate) fn unify_eff(&mut self, a: Effect, b: Effect) -> Result<(), TypeError> {
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
                self.uni.eff_subst.insert(v, Effect::Io);
                Ok(())
            }
            (Effect::Var(v), Effect::Var(w)) => {
                if v != w {
                    self.uni.eff_subst.insert(v, Effect::Var(w));
                }
                Ok(())
            }
        }
    }

    /// Join types at merge points (`if` arms, `var` assign). Function effects use
    /// lub (`Pure ⊔ Io = Io`) instead of equality so IO cannot be lost.
    pub(crate) fn join_types(
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
            (Type::Set(a), Type::Set(b)) => Ok(Type::Set(Box::new(self.join_types(*a, *b, span)?))),
            (Type::Task(a), Type::Task(b)) => {
                Ok(Type::Task(Box::new(self.join_types(*a, *b, span)?)))
            }
            (Type::Channel(a), Type::Channel(b)) => {
                Ok(Type::Channel(Box::new(self.join_types(*a, *b, span)?)))
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
            (Type::TuplePrefix(a), Type::TuplePrefix(b)) => {
                let n = a.len().max(b.len());
                let mut ts = Vec::with_capacity(n);
                for i in 0..n {
                    match (a.get(i), b.get(i)) {
                        (Some(x), Some(y)) => {
                            ts.push(self.join_types(x.clone(), y.clone(), span)?)
                        }
                        (Some(x), None) | (None, Some(x)) => ts.push(x.clone()),
                        (None, None) => unreachable!(),
                    }
                }
                Ok(Type::TuplePrefix(ts))
            }
            (Type::TuplePrefix(p), Type::Tuple(t)) | (Type::Tuple(t), Type::TuplePrefix(p)) => {
                if t.len() < p.len() {
                    return Err(at(span, "tuple arity mismatch"));
                }
                for (x, y) in p.iter().zip(t.iter()) {
                    self.join_types(x.clone(), y.clone(), span)?;
                }
                Ok(Type::Tuple(t))
            }
            (a, b) => {
                self.unify_at(span, a.clone(), b)?;
                Ok(self.prune(a))
            }
        }
    }

    pub(crate) fn rebind(&mut self, name: &str, ty: Type) -> Result<(), TypeError> {
        self.rebind_scheme(name, Scheme::mono(ty))
    }

    pub(crate) fn rebind_scheme(&mut self, name: &str, scheme: Scheme) -> Result<(), TypeError> {
        for scope in self.scopes.env.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), scheme);
                return Ok(());
            }
        }
        Err(TypeError::Message(format!("unbound `{name}` in rebind")))
    }

    pub(crate) fn unify(&mut self, a: Type, b: Type) -> Result<(), TypeError> {
        let a = self.prune(a);
        let b = self.prune(b);
        match (a, b) {
            (Type::Var(v), Type::Var(u)) if v == u => Ok(()),
            (Type::Var(v), t) | (t, Type::Var(v)) => {
                // Allow equi-recursive ADTs (`α ~ Expr[α]`); still reject
                // cycles through List/Fun/Tuple/Map/etc.
                if occurs_rigid(v, &t) {
                    return Err(TypeError::Message("infinite type".into()));
                }
                self.check_num_bind(v, &t)?;
                self.check_ord_bind(v, &t)?;
                self.check_eq_bind(v, &t)?;
                self.check_len_bind(v, &t)?;
                self.check_concat_bind(v, &t)?;
                self.check_contains_bind(v, &t)?;
                self.check_set_bind(v, &t)?;
                self.check_elems_bind(v, &t)?;
                self.check_take_bind(v, &t)?;
                self.check_trait_bind(v, &t)?;
                if let Type::Var(u) = &t {
                    if self.uni.num_vars.contains(&v) {
                        self.uni.num_vars.insert(*u);
                    }
                    if self.uni.num_vars.contains(u) {
                        self.uni.num_vars.insert(v);
                    }
                    if self.uni.ord_vars.contains(&v) {
                        self.uni.ord_vars.insert(*u);
                    }
                    if self.uni.ord_vars.contains(u) {
                        self.uni.ord_vars.insert(v);
                    }
                    if self.uni.eq_vars.contains(&v) {
                        self.uni.eq_vars.insert(*u);
                    }
                    if self.uni.eq_vars.contains(u) {
                        self.uni.eq_vars.insert(v);
                    }
                    if self.uni.len_vars.contains(&v) {
                        self.uni.len_vars.insert(*u);
                    }
                    if self.uni.len_vars.contains(u) {
                        self.uni.len_vars.insert(v);
                    }
                    if self.uni.concat_vars.contains(&v) {
                        self.uni.concat_vars.insert(*u);
                    }
                    if self.uni.concat_vars.contains(u) {
                        self.uni.concat_vars.insert(v);
                    }
                    if self.uni.contains_vars.contains(&v) {
                        self.uni.contains_vars.insert(*u);
                    }
                    if self.uni.contains_vars.contains(u) {
                        self.uni.contains_vars.insert(v);
                    }
                    if self.uni.set_vars.contains(&v) {
                        self.uni.set_vars.insert(*u);
                    }
                    if self.uni.set_vars.contains(u) {
                        self.uni.set_vars.insert(v);
                    }
                    if self.uni.elems_vars.contains(&v) {
                        self.uni.elems_vars.insert(*u);
                    }
                    if self.uni.elems_vars.contains(u) {
                        self.uni.elems_vars.insert(v);
                    }
                    if self.uni.take_vars.contains(&v) {
                        self.uni.take_vars.insert(*u);
                    }
                    if self.uni.take_vars.contains(u) {
                        self.uni.take_vars.insert(v);
                    }
                }
                self.uni.subst.insert(v, t);
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
            (Type::Task(a), Type::Task(b)) => self.unify(*a, *b),
            (Type::Channel(a), Type::Channel(b)) => self.unify(*a, *b),
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
            (Type::TuplePrefix(p), Type::Tuple(t)) | (Type::Tuple(t), Type::TuplePrefix(p)) => {
                if t.len() < p.len() {
                    return Err(TypeError::Message(format!(
                        "tuple too short for positional projection (need ≥ {}, got {})",
                        p.len(),
                        t.len()
                    )));
                }
                for (x, y) in p.into_iter().zip(t) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Type::TuplePrefix(a), Type::TuplePrefix(b)) => {
                // Keep the longer prefix (at-least-N); min would drop `.1` after `.0`.
                let n = a.len().max(b.len());
                for i in 0..n {
                    match (a.get(i), b.get(i)) {
                        (Some(x), Some(y)) => self.unify(x.clone(), y.clone())?,
                        (Some(_), None) | (None, Some(_)) => {}
                        (None, None) => unreachable!(),
                    }
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
            (a, b) => Err(TypeError::Message(format!("type mismatch: {a:?} vs {b:?}"))),
        }
    }

    pub(crate) fn unify_at(
        &mut self,
        span: lumia_syntax::Span,
        a: Type,
        b: Type,
    ) -> Result<(), TypeError> {
        self.unify(a, b).map_err(|e| locate(span, e))
    }
}

pub(crate) fn occurs(v: u32, ty: &Type) -> bool {
    match ty {
        Type::Var(u) => *u == v,
        Type::Fun(ps, r, _) => ps.iter().any(|p| occurs(v, p)) || occurs(v, r),
        Type::List(t) | Type::Set(t) | Type::Task(t) | Type::Channel(t) => occurs(v, t),
        Type::Map(k, t) => occurs(v, k) || occurs(v, t),
        Type::Adt { params, .. } => params.iter().any(|p| occurs(v, p)),
        Type::Tuple(ts) | Type::TuplePrefix(ts) => ts.iter().any(|p| occurs(v, p)),
        _ => false,
    }
}

/// Occurs check that still rejects infinite List/Fun/Tuple/… types, but allows
/// a type variable to appear under `Adt` constructors (equi-recursive trees).
fn occurs_rigid(v: u32, ty: &Type) -> bool {
    match ty {
        Type::Var(u) => *u == v,
        Type::Adt { params, .. } => params.iter().any(|p| occurs_rigid_in_adt_arg(v, p)),
        Type::Fun(ps, r, _) => ps.iter().any(|p| occurs(v, p)) || occurs(v, r),
        Type::List(t) | Type::Set(t) | Type::Task(t) | Type::Channel(t) => occurs(v, t),
        Type::Map(k, t) => occurs(v, k) || occurs(v, t),
        Type::Tuple(ts) | Type::TuplePrefix(ts) => ts.iter().any(|p| occurs(v, p)),
        _ => false,
    }
}

fn occurs_rigid_in_adt_arg(v: u32, ty: &Type) -> bool {
    match ty {
        // `α` as an ADT type argument is the equi-recursive spine.
        Type::Var(_) => false,
        Type::Adt { params, .. } => params.iter().any(|p| occurs_rigid_in_adt_arg(v, p)),
        other => occurs(v, other),
    }
}

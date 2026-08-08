//! Core IR — ANF / SSA-ish form used by optimization and codegen.

use lumia_hir::{Builtin, Expr as HirExpr, Item, Module as HirModule};
use lumia_syntax::{BinOp, UnOp};
use lumia_ty::{Effect, Type};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub u32);

#[derive(Debug, Clone)]
pub struct CoreModule {
    pub name: String,
    pub functions: Vec<CoreFun>,
}

#[derive(Debug, Clone)]
pub struct CoreFun {
    pub name: String,
    pub params: Vec<Local>,
    pub param_names: Vec<String>,
    /// Parameter types (for float ABI / future typed SSA).
    pub param_tys: Vec<Type>,
    pub body: Block,
    pub ret_ty: Type,
    pub effect: Effect,
    pub is_main: bool,
    /// Transparent Memo `T_f` (DESIGN §7.5.1-B). `None` = capacity 0.
    pub memo: Option<MemoTf>,
    /// When set, this is a C ABI import (`foreign`); no body emitted.
    pub external: Option<String>,
    /// Locals that may escape (always filled by EscapePass before ReprSelect).
    pub escaping: HashSet<Local>,
}

/// Bounded cross-call memo table — one mechanism, representation is a parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoTf {
    /// Fixed small associative table (hot keys / irregular args).
    Slots { id: u32 },
    /// Dense Int domain `[0, CAP)` — prefer for structural recursion on `n`.
    DenseInt { id: u32 },
}

#[derive(Debug, Clone)]
pub struct Block {
    pub params: Vec<Local>,
    pub ops: Vec<Op>,
    /// Result local (or unit)
    pub result: Option<Local>,
}

#[derive(Debug, Clone)]
pub enum Op {
    Let {
        local: Local,
        value: Value,
        /// Pure region marker for §7.4.1
        pure_region: bool,
    },
    /// Effectful statement (no bind)
    Effect {
        value: Value,
    },
    Assign {
        name: String,
        value: Local,
    },
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Unit,
    Local(Local),
    /// Named mutable/immutable binding load
    Name(String),
    Binary {
        op: BinOp,
        left: Local,
        right: Local,
    },
    Unary {
        op: UnOp,
        operand: Local,
    },
    Call {
        fun: String,
        args: Vec<Local>,
    },
    /// Call through a function pointer (first-class / lifted lambda).
    IndirectCall {
        callee: Local,
        args: Vec<Local>,
    },
    Builtin {
        name: Builtin,
        args: Vec<Local>,
    },
    If {
        cond: Local,
        then_block: Box<Block>,
        else_block: Box<Block>,
    },
    /// While-style: `header` recomputes condition; `body`; `latch` runs before next header
    /// (`continue` → latch; normal body end → latch).
    Loop {
        header: Box<Block>,
        body: Box<Block>,
        latch: Box<Block>,
    },
    Lambda {
        params: Vec<Local>,
        body: Box<Block>,
    },
    /// Pointer to a known Core/LLVM function (as i64).
    FunRef(String),
    /// Representation hint after lowering (default path)
    AllocList {
        elems: Vec<Local>,
        repr: ListRepr,
    },
    AllocSet {
        elems: Vec<Local>,
    },
    /// Empty or flat key/value locals: [k0,v0,k1,v1,...]
    AllocMap {
        flat_pairs: Vec<Local>,
        repr: MapRepr,
    },
    /// Sum type: `[tag:i64][field0]…`
    AllocAdt {
        tag: i64,
        fields: Vec<Local>,
    },
    /// Heap closure: `[fn_ptr:i64][cap0]…` — `fun` takes `(env, …params)`.
    AllocClosure {
        fun: String,
        captures: Vec<Local>,
    },
    /// Load capture word `index` from closure env (`env` is the heap ptr as i64).
    /// `as_float` restores IEEE bits to f64 in codegen (captures are stored as i64).
    ClosureCap {
        env: Local,
        index: u32,
        as_float: bool,
    },
}

/// List representation hint on `AllocList` (§3.5 / §7.1.1).
/// Runtime Iota ranges use `TYPE_LIST_IOTA` via `lumia_range`, not this enum.
/// Deforestation lives in HIR (`try_fuse_hof_*`), not as an AllocList tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRepr {
    /// Default heap `[len][elems…]`.
    HeapList,
    /// Empty list only — codegen emits immortal `lumia_list_empty`.
    LitList,
}

/// Map default path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapRepr {
    HashOrdered,
    SmallMap,
}

struct LowerCtx {
    next: u32,
    name_to_local: HashMap<String, Local>,
    mutables: std::collections::HashSet<String>,
    toplevel_funs: std::collections::HashSet<String>,
    toplevel_vals: std::collections::HashSet<String>,
}

impl LowerCtx {
    fn new(
        toplevel_funs: std::collections::HashSet<String>,
        toplevel_vals: std::collections::HashSet<String>,
    ) -> Self {
        Self {
            next: 0,
            name_to_local: HashMap::new(),
            mutables: std::collections::HashSet::new(),
            toplevel_funs,
            toplevel_vals,
        }
    }

    fn fresh(&mut self) -> Local {
        let l = Local(self.next);
        self.next += 1;
        l
    }

    fn bind_name(&mut self, name: String, local: Local) {
        self.name_to_local.insert(name, local);
    }

    fn bind_mutable(&mut self, name: String, local: Local) {
        self.mutables.insert(name.clone());
        self.bind_name(name, local);
    }

    /// Snapshot of name bindings (not `next` — locals stay unique across scopes).
    fn save_bindings(&self) -> (HashMap<String, Local>, HashSet<String>) {
        (self.name_to_local.clone(), self.mutables.clone())
    }

    fn restore_bindings(&mut self, saved: (HashMap<String, Local>, HashSet<String>)) {
        self.name_to_local = saved.0;
        self.mutables = saved.1;
    }
}

pub fn lower_hir(module: &HirModule, fun_types: &HashMap<String, Type>) -> CoreModule {
    let toplevel_funs: std::collections::HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fun(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    let toplevel_vals: std::collections::HashSet<String> = module
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Val { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let mut functions = vec![];
    for item in &module.items {
        match item {
            Item::Fun(f) => {
                let mut ctx = LowerCtx::new(toplevel_funs.clone(), toplevel_vals.clone());
                let mut params = vec![];
                for p in &f.params {
                    let l = ctx.fresh();
                    ctx.bind_name(p.clone(), l);
                    params.push(l);
                }
                let (body, _) = lower_expr_block(&mut ctx, &f.body);
                let (ret_ty, effect, param_tys) = match fun_types.get(&f.name) {
                    Some(Type::Fun(ps, r, e)) => ((**r).clone(), *e, ps.clone()),
                    _ => (
                        Type::Unit,
                        if f.is_main {
                            Effect::io()
                        } else {
                            Effect::pure()
                        },
                        vec![Type::Int; f.params.len()],
                    ),
                };
                functions.push(CoreFun {
                    name: f.name.clone(),
                    params,
                    param_names: f.params.clone(),
                    param_tys,
                    body,
                    ret_ty,
                    effect,
                    is_main: f.is_main,
                    memo: None,
                    external: f.external.clone(),
                    escaping: HashSet::new(),
                });
            }
            Item::Val { name, body } => {
                // Module-level `val` → zero-arg getter `__val_<name>` (pure).
                // Ret type must match inference so codegen roots heap returns.
                let getter = format!("__val_{name}");
                let ret_ty = match fun_types.get(&getter).or_else(|| fun_types.get(name)) {
                    Some(Type::Fun(_, r, _)) => (**r).clone(),
                    Some(t) => t.clone(),
                    None => Type::Int,
                };
                let mut ctx = LowerCtx::new(toplevel_funs.clone(), toplevel_vals.clone());
                let (body, _) = lower_expr_block(&mut ctx, body);
                functions.push(CoreFun {
                    name: getter,
                    params: vec![],
                    param_names: vec![],
                    param_tys: vec![],
                    body,
                    ret_ty,
                    effect: Effect::pure(),
                    is_main: false,
                    memo: None,
                    external: None,
                    escaping: HashSet::new(),
                });
            }
        }
    }
    let mut core = CoreModule {
        name: module.name.clone(),
        functions,
    };
    lift_lambdas(&mut core);
    directize_funref_calls(&mut core);
    core
}

/// When a local is bound to `FunRef(name)`, rewrite `IndirectCall` of that local
/// into a direct `Call` so float/int ABI (`param_tys` / `ret_ty`) applies.
fn directize_funref_calls(module: &mut CoreModule) {
    for fun in &mut module.functions {
        directize_block(&mut fun.body);
    }
}

fn directize_block(block: &mut Block) {
    let mut funref_of: HashMap<u32, String> = HashMap::new();
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                directize_value(value, &funref_of);
                walk_nested_blocks_directize(value);
                if let Value::FunRef(name) = value {
                    funref_of.insert(local.0, name.clone());
                } else {
                    funref_of.remove(&local.0);
                }
            }
            Op::Effect { value } => {
                directize_value(value, &funref_of);
                walk_nested_blocks_directize(value);
            }
            Op::Assign { .. } | Op::Break | Op::Continue => {}
        }
    }
}

fn walk_nested_blocks_directize(value: &mut Value) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            directize_block(then_block);
            directize_block(else_block);
        }
        Value::Loop {
            header,
            body,
            latch,
            ..
        } => {
            directize_block(header);
            directize_block(body);
            directize_block(latch);
        }
        Value::Lambda { body, .. } => directize_block(body),
        _ => {}
    }
}

fn directize_value(value: &mut Value, funref_of: &HashMap<u32, String>) {
    if let Value::IndirectCall { callee, args } = value {
        if let Some(name) = funref_of.get(&callee.0) {
            *value = Value::Call {
                fun: name.clone(),
                args: args.clone(),
            };
        }
    }
}

/// Infer per-parameter / return ABI for lifted lambdas.
/// Avoids the old bug: “body mentions any float ⇒ every param is Float”.
fn lambda_param_ret_tys(params: &[Local], body: &Block) -> (Vec<Type>, Type) {
    let float_params = params_used_as_float(body, params);
    let param_tys = params
        .iter()
        .map(|p| {
            if float_params.contains(&p.0) {
                Type::Float
            } else {
                Type::Int
            }
        })
        .collect();
    let ret_ty = if block_result_is_float(body) {
        Type::Float
    } else if block_result_may_heap_with_params(body, params) {
        // Conservative heap marker so codegen roots the Call result (§GC).
        Type::List(Box::new(Type::Int))
    } else {
        Type::Int
    };
    (param_tys, ret_ty)
}

fn params_used_as_float(block: &Block, params: &[Local]) -> HashSet<u32> {
    let param_set: HashSet<u32> = params.iter().map(|p| p.0).collect();
    let mut float_locals: HashSet<u32> = HashSet::new();
    let mut used: HashSet<u32> = HashSet::new();
    mark_float_uses(block, &param_set, &mut float_locals, &mut used);
    used
}

fn mark_float_uses(
    block: &Block,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
) {
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                mark_float_in_value(value, params, float_locals, used);
                if value_is_float_producing(value, float_locals) {
                    float_locals.insert(local.0);
                }
            }
            Op::Effect { value } => mark_float_in_value(value, params, float_locals, used),
            _ => {}
        }
    }
}

fn mark_float_in_value(
    v: &Value,
    params: &HashSet<u32>,
    float_locals: &mut HashSet<u32>,
    used: &mut HashSet<u32>,
) {
    match v {
        Value::Binary { left, right, .. } => {
            let lf = float_locals.contains(&left.0);
            let rf = float_locals.contains(&right.0);
            if lf || rf {
                touch_param(left.0, params, used);
                touch_param(right.0, params, used);
            }
        }
        Value::Unary { operand, .. } => {
            if float_locals.contains(&operand.0) {
                touch_param(operand.0, params, used);
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            mark_float_uses(then_block, params, float_locals, used);
            mark_float_uses(else_block, params, float_locals, used);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            mark_float_uses(header, params, float_locals, used);
            mark_float_uses(body, params, float_locals, used);
            mark_float_uses(latch, params, float_locals, used);
        }
        _ => {}
    }
}

fn touch_param(id: u32, params: &HashSet<u32>, used: &mut HashSet<u32>) {
    if params.contains(&id) {
        used.insert(id);
    }
}

fn value_is_float_producing(v: &Value, float_locals: &HashSet<u32>) -> bool {
    match v {
        Value::Float(_) => true,
        Value::Local(Local(id)) => float_locals.contains(id),
        Value::ClosureCap { as_float: true, .. } => true,
        Value::Binary { left, right, .. } => {
            float_locals.contains(&left.0) || float_locals.contains(&right.0)
        }
        Value::Unary { operand, .. } => float_locals.contains(&operand.0),
        _ => false,
    }
}

fn block_result_is_float(block: &Block) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut float_locals: HashSet<u32> = HashSet::new();
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if value_is_float_producing(value, &float_locals) || matches!(value, Value::Float(_)) {
                float_locals.insert(local.0);
            }
            if matches!(value, Value::Float(_)) {
                float_locals.insert(local.0);
            }
            // Propagate through float binaries more carefully:
            if let Value::Binary { left, right, .. } = value {
                if float_locals.contains(&left.0) || float_locals.contains(&right.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Local(Local(src)) = value {
                if float_locals.contains(src) {
                    float_locals.insert(local.0);
                }
            }
        }
    }
    float_locals.contains(&r)
}


/// Locals that hold Float values in `block` (for closure-capture ABI).
fn compute_float_locals_in_block(block: &Block) -> HashSet<u32> {
    let mut float_locals: HashSet<u32> = HashSet::new();
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if value_is_float_producing(value, &float_locals) || matches!(value, Value::Float(_)) {
                float_locals.insert(local.0);
            }
            if let Value::Binary { left, right, .. } = value {
                if float_locals.contains(&left.0) || float_locals.contains(&right.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::Local(Local(src)) = value {
                if float_locals.contains(src) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::ClosureCap { as_float: true, .. } = value {
                float_locals.insert(local.0);
            }
            if let Value::Unary { operand, .. } = value {
                if float_locals.contains(&operand.0) {
                    float_locals.insert(local.0);
                }
            }
            if let Value::If {
                then_block,
                else_block,
                ..
            } = value
            {
                float_locals.extend(compute_float_locals_in_block(then_block));
                float_locals.extend(compute_float_locals_in_block(else_block));
            }
        }
    }
    float_locals
}

/// Whether the block result may be a heap pointer. `extra_params` covers lambda
/// formals that live on `Value::Lambda.params` rather than `body.params`.
fn block_result_may_heap_with_params(block: &Block, extra_params: &[Local]) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut params: HashSet<u32> = block.params.iter().map(|p| p.0).collect();
    params.extend(extra_params.iter().map(|p| p.0));
    local_may_heap(block, r, &params, &mut HashSet::new())
}

/// Follow `let x = y` aliases. Params are treated as maybe-heap so identity
/// lambdas like `{ s -> s }` keep a heap `ret_ty` for GC rooting at call sites.
fn local_may_heap(
    block: &Block,
    id: u32,
    params: &HashSet<u32>,
    seen: &mut HashSet<u32>,
) -> bool {
    if !seen.insert(id) {
        return true;
    }
    if params.contains(&id) {
        return true;
    }
    for op in &block.ops {
        if let Op::Let { local, value, .. } = op {
            if local.0 == id {
                return value_may_heap(block, value, params, seen);
            }
        }
    }
    false
}

fn value_may_heap(
    block: &Block,
    v: &Value,
    params: &HashSet<u32>,
    seen: &mut HashSet<u32>,
) -> bool {
    match v {
        Value::Local(Local(id)) => local_may_heap(block, *id, params, seen),
        Value::String(_)
        | Value::Char(_)
        | Value::AllocList { .. }
        | Value::AllocSet { .. }
        | Value::AllocMap { .. }
        | Value::AllocAdt { .. }
        | Value::AllocClosure { .. }
        | Value::ClosureCap { .. }
        | Value::FunRef(_) => true,
        Value::Builtin { name, .. } => !matches!(
            name,
            Builtin::ListLen
                | Builtin::Contains
                | Builtin::Println
                | Builtin::PrintlnInt
                | Builtin::PrintlnStr
                | Builtin::Assert
        ),
        Value::Call { .. } | Value::IndirectCall { .. } => true,
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            // Nested blocks inherit lambda/outer params for alias tracking.
            result_may_heap_inherited(then_block, params)
                || result_may_heap_inherited(else_block, params)
        }
        _ => false,
    }
}

fn result_may_heap_inherited(block: &Block, inherited: &HashSet<u32>) -> bool {
    let Some(Local(r)) = block.result else {
        return false;
    };
    let mut params = inherited.clone();
    params.extend(block.params.iter().map(|p| p.0));
    local_may_heap(block, r, &params, &mut HashSet::new())
}

/// Lift nested `Value::Lambda` to top-level `__lam_N` functions.
/// Captures (free locals / outer `var` loads) become a heap closure env.
fn lift_lambdas(module: &mut CoreModule) {
    let mut extras = Vec::new();
    let mut id = 0u32;
    let mut next_local = max_local_in_module(module).saturating_add(1);
    for fun in &mut module.functions {
        let mut float_locals = compute_float_locals_in_block(&fun.body);
        for (i, ty) in fun.param_tys.iter().enumerate() {
            if matches!(ty, Type::Float) {
                if let Some(p) = fun.params.get(i) {
                    float_locals.insert(p.0);
                }
            }
        }
        lift_block(
            &mut fun.body,
            &mut extras,
            &mut id,
            &mut next_local,
            &mut float_locals,
        );
    }
    module.functions.append(&mut extras);
}

fn max_local_in_module(module: &CoreModule) -> u32 {
    let mut max = 0u32;
    for fun in &module.functions {
        max = max.max(max_local_in_fun(fun));
    }
    max
}

/// Highest `Local` id used in a function (params + body).
pub fn max_local_in_fun(fun: &CoreFun) -> u32 {
    let mut max = 0u32;
    for p in &fun.params {
        max = max.max(p.0);
    }
    max.max(max_local_in_block(&fun.body))
}

pub fn max_local_in_block(block: &Block) -> u32 {
    let mut max = 0u32;
    for p in &block.params {
        max = max.max(p.0);
    }
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                max = max.max(local.0);
                max = max.max(max_local_in_value(value));
            }
            Op::Effect { value, .. } => {
                max = max.max(max_local_in_value(value));
            }
            Op::Assign { value, .. } => max = max.max(value.0),
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = &block.result {
        max = max.max(r.0);
    }
    max
}

fn max_local_in_value(value: &Value) -> u32 {
    match value {
        Value::Local(l) => l.0,
        Value::Binary { left, right, .. } => left.0.max(right.0),
        Value::Unary { operand, .. } => operand.0,
        Value::Call { args, .. }
        | Value::Builtin { args, .. }
        | Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure {
            captures: args, ..
        } => args.iter().map(|l| l.0).max().unwrap_or(0),
        Value::IndirectCall { callee, args } => args
            .iter()
            .map(|l| l.0)
            .max()
            .unwrap_or(0)
            .max(callee.0),
        Value::If {
            cond,
            then_block,
            else_block,
        } => cond
            .0
            .max(max_local_in_block(then_block))
            .max(max_local_in_block(else_block)),
        Value::Loop {
            header,
            body,
            latch,
        } => max_local_in_block(header)
            .max(max_local_in_block(body))
            .max(max_local_in_block(latch)),
        Value::Lambda { params, body } => params
            .iter()
            .map(|l| l.0)
            .max()
            .unwrap_or(0)
            .max(max_local_in_block(body)),
        Value::ClosureCap { env, .. } => env.0,
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::Name(_)
        | Value::FunRef(_) => 0,
    }
}

fn lift_block(
    block: &mut Block,
    extras: &mut Vec<CoreFun>,
    id: &mut u32,
    next_local: &mut u32,
    float_locals: &mut HashSet<u32>,
) {
    let mut new_ops = Vec::with_capacity(block.ops.len());
    for mut op in std::mem::take(&mut block.ops) {
        match &mut op {
            Op::Let { value, pure_region, .. } => {
                let mut prelude = Vec::new();
                lift_value(
                    value,
                    extras,
                    id,
                    next_local,
                    &mut prelude,
                    *pure_region,
                    float_locals,
                );
                new_ops.append(&mut prelude);
            }
            Op::Effect { value, .. } => {
                let mut prelude = Vec::new();
                lift_value(
                    value,
                    extras,
                    id,
                    next_local,
                    &mut prelude,
                    true,
                    float_locals,
                );
                new_ops.append(&mut prelude);
            }
            Op::Assign { .. } | Op::Break | Op::Continue => {}
        }
        new_ops.push(op);
    }
    block.ops = new_ops;
}

fn lift_value(
    value: &mut Value,
    extras: &mut Vec<CoreFun>,
    id: &mut u32,
    next_local: &mut u32,
    prelude: &mut Vec<Op>,
    pure_region: bool,
    float_locals: &mut HashSet<u32>,
) {
    match value {
        Value::Lambda { params, body } => {
            lift_block(body, extras, id, next_local, float_locals);
            let (free_locals, free_names) = analyze_captures(body, params);
            let name = format!("__lam_{id}");
            *id += 1;

            let mut captures = Vec::new();
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let mut name_remap: HashMap<String, Local> = HashMap::new();

            for fl in &free_locals {
                captures.push(*fl);
            }
            for n in &free_names {
                let tmp = Local(*next_local);
                *next_local += 1;
                prelude.push(Op::Let {
                    local: tmp,
                    value: Value::Name(n.clone()),
                    pure_region,
                });
                captures.push(tmp);
                name_remap.insert(n.clone(), tmp);
            }

            if captures.is_empty() {
                let param_names: Vec<String> =
                    (0..params.len()).map(|i| format!("p{i}")).collect();
                let (param_tys, ret_ty) = lambda_param_ret_tys(params, body);
                extras.push(CoreFun {
                    name: name.clone(),
                    params: params.clone(),
                    param_names,
                    param_tys,
                    body: *body.clone(),
                    ret_ty,
                    effect: Effect::pure(),
                    is_main: false,
                    memo: None,
                    external: None,
                escaping: std::collections::HashSet::new(),
                });
                *value = Value::FunRef(name);
                return;
            }

            let env = Local(*next_local);
            *next_local += 1;
            let mut new_body = *body.clone();
            // Map each capture slot → a fresh local loaded from env at entry.
            let mut load_ops = Vec::new();
            for (i, cap_src) in captures.iter().enumerate() {
                let loaded = Local(*next_local);
                *next_local += 1;
                let as_float = float_locals.contains(&cap_src.0);
                if as_float {
                    float_locals.insert(loaded.0);
                }
                load_ops.push(Op::Let {
                    local: loaded,
                    value: Value::ClosureCap {
                        env,
                        index: i as u32,
                        as_float,
                    },
                    pure_region: true,
                });
                let name_hit = name_remap
                    .iter()
                    .find(|(_, l)| l.0 == cap_src.0)
                    .map(|(n, _)| n.clone());
                if let Some(name) = name_hit {
                    name_remap.insert(name, loaded);
                } else {
                    remap.insert(cap_src.0, loaded.0);
                }
            }
            rewrite_block_locals(&mut new_body, &remap);
            rewrite_block_names(&mut new_body, &name_remap);

            let mut ops = load_ops;
            ops.append(&mut new_body.ops);
            new_body.ops = ops;

            let mut fun_params = vec![env];
            fun_params.extend(params.iter().copied());
            let mut param_names = vec!["env".into()];
            param_names.extend((0..params.len()).map(|i| format!("p{i}")));

            let (user_param_tys, ret_ty) = lambda_param_ret_tys(params, &new_body);
            extras.push(CoreFun {
                name: name.clone(),
                params: fun_params,
                param_names,
                param_tys: {
                    let mut tys = vec![Type::Int]; // env pointer bits
                    tys.extend(user_param_tys);
                    tys
                },
                body: new_body,
                ret_ty,
                effect: Effect::pure(),
                is_main: false,
                memo: None,
                external: None,
            escaping: std::collections::HashSet::new(),
            });
            *value = Value::AllocClosure {
                fun: name,
                captures,
            };
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            lift_block(then_block, extras, id, next_local, float_locals);
            lift_block(else_block, extras, id, next_local, float_locals);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            lift_block(header, extras, id, next_local, float_locals);
            lift_block(body, extras, id, next_local, float_locals);
            lift_block(latch, extras, id, next_local, float_locals);
        }
        _ => {}
    }
}

fn analyze_captures(body: &Block, params: &[Local]) -> (Vec<Local>, Vec<String>) {
    let mut defined = std::collections::HashSet::new();
    for p in params {
        defined.insert(p.0);
    }
    collect_defined_locals(body, &mut defined);
    let mut used_locals = std::collections::HashSet::new();
    let mut used_names = std::collections::HashSet::new();
    collect_uses(body, &mut used_locals, &mut used_names);
    let mut free_locals: Vec<Local> = used_locals
        .into_iter()
        .filter(|id| !defined.contains(id))
        .map(Local)
        .collect();
    free_locals.sort_by_key(|l| l.0);
    let mut free_names: Vec<String> = used_names.into_iter().collect();
    free_names.sort();
    (free_locals, free_names)
}

fn collect_defined_locals(block: &Block, defined: &mut std::collections::HashSet<u32>) {
    for p in &block.params {
        defined.insert(p.0);
    }
    for op in &block.ops {
        match op {
            Op::Let { local, value, .. } => {
                defined.insert(local.0);
                collect_defined_in_value(value, defined);
            }
            Op::Effect { value, .. } => collect_defined_in_value(value, defined),
            Op::Assign { .. } | Op::Break | Op::Continue => {}
        }
    }
}

fn collect_defined_in_value(value: &Value, defined: &mut std::collections::HashSet<u32>) {
    match value {
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            collect_defined_locals(then_block, defined);
            collect_defined_locals(else_block, defined);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_defined_locals(header, defined);
            collect_defined_locals(body, defined);
            collect_defined_locals(latch, defined);
        }
        Value::Lambda { params, body } => {
            for p in params {
                defined.insert(p.0);
            }
            collect_defined_locals(body, defined);
        }
        _ => {}
    }
}

fn collect_uses(
    block: &Block,
    locals: &mut std::collections::HashSet<u32>,
    names: &mut std::collections::HashSet<String>,
) {
    for op in &block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                collect_uses_in_value(value, locals, names);
            }
            Op::Assign { value, .. } => {
                locals.insert(value.0);
            }
            Op::Break | Op::Continue => {}
        }
    }
    if let Some(r) = &block.result {
        locals.insert(r.0);
    }
}

fn collect_uses_in_value(
    value: &Value,
    locals: &mut std::collections::HashSet<u32>,
    names: &mut std::collections::HashSet<String>,
) {
    match value {
        Value::Local(l) => {
            locals.insert(l.0);
        }
        Value::Name(n) => {
            names.insert(n.clone());
        }
        Value::Binary { left, right, .. } => {
            locals.insert(left.0);
            locals.insert(right.0);
        }
        Value::Unary { operand, .. } => {
            locals.insert(operand.0);
        }
        Value::Call { args, .. }
        | Value::Builtin { args, .. }
        | Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure {
            captures: args, ..
        } => {
            for a in args {
                locals.insert(a.0);
            }
        }
        Value::IndirectCall { callee, args } => {
            locals.insert(callee.0);
            for a in args {
                locals.insert(a.0);
            }
        }
        Value::If {
            cond,
            then_block,
            else_block,
        } => {
            locals.insert(cond.0);
            collect_uses(then_block, locals, names);
            collect_uses(else_block, locals, names);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            collect_uses(header, locals, names);
            collect_uses(body, locals, names);
            collect_uses(latch, locals, names);
        }
        Value::Lambda { body, .. } => {
            // Nested lambdas are lifted first; remaining free uses inside are their problem.
            // Still walk in case lift order left a Lambda (shouldn't).
            collect_uses(body, locals, names);
        }
        Value::ClosureCap { env, .. } => {
            locals.insert(env.0);
        }
        Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_) => {}
    }
}

/// Remap SSA locals in-place (used by opt inlining / lifting).
pub fn rewrite_block_locals(block: &mut Block, remap: &HashMap<u32, u32>) {
    if remap.is_empty() {
        return;
    }
    let map_l = |l: &mut Local| {
        if let Some(&r) = remap.get(&l.0) {
            *l = Local(r);
        }
    };
    for p in &mut block.params {
        map_l(p);
    }
    if let Some(r) = &mut block.result {
        map_l(r);
    }
    for op in &mut block.ops {
        match op {
            Op::Let { local, value, .. } => {
                map_l(local);
                rewrite_value_locals(value, remap);
            }
            Op::Effect { value, .. } => rewrite_value_locals(value, remap),
            Op::Assign { value, .. } => map_l(value),
            Op::Break | Op::Continue => {}
        }
    }
}

fn rewrite_value_locals(value: &mut Value, remap: &HashMap<u32, u32>) {
    let map_l = |l: &mut Local| {
        if let Some(&r) = remap.get(&l.0) {
            *l = Local(r);
        }
    };
    match value {
        Value::Local(l) => map_l(l),
        Value::Binary { left, right, .. } => {
            map_l(left);
            map_l(right);
        }
        Value::Unary { operand, .. } => map_l(operand),
        Value::Call { args, .. }
        | Value::Builtin { args, .. }
        | Value::AllocList { elems: args, .. }
        | Value::AllocSet { elems: args }
        | Value::AllocMap {
            flat_pairs: args, ..
        }
        | Value::AllocAdt { fields: args, .. }
        | Value::AllocClosure {
            captures: args, ..
        } => {
            for a in args {
                map_l(a);
            }
        }
        Value::IndirectCall { callee, args } => {
            map_l(callee);
            for a in args {
                map_l(a);
            }
        }
        Value::If {
            cond,
            then_block,
            else_block,
        } => {
            map_l(cond);
            rewrite_block_locals(then_block, remap);
            rewrite_block_locals(else_block, remap);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_block_locals(header, remap);
            rewrite_block_locals(body, remap);
            rewrite_block_locals(latch, remap);
        }
        Value::Lambda { params, body } => {
            for p in params {
                map_l(p);
            }
            rewrite_block_locals(body, remap);
        }
        Value::ClosureCap { env, .. } => map_l(env),
        Value::Name(_)
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_)
        | Value::Char(_)
        | Value::Unit
        | Value::FunRef(_) => {}
    }
}

fn rewrite_block_names(block: &mut Block, name_remap: &HashMap<String, Local>) {
    if name_remap.is_empty() {
        return;
    }
    for op in &mut block.ops {
        match op {
            Op::Let { value, .. } | Op::Effect { value, .. } => {
                rewrite_value_names(value, name_remap);
            }
            Op::Assign { .. } | Op::Break | Op::Continue => {}
        }
    }
}

fn rewrite_value_names(value: &mut Value, name_remap: &HashMap<String, Local>) {
    match value {
        Value::Name(n) => {
            if let Some(l) = name_remap.get(n) {
                *value = Value::Local(*l);
            }
        }
        Value::If {
            then_block,
            else_block,
            ..
        } => {
            rewrite_block_names(then_block, name_remap);
            rewrite_block_names(else_block, name_remap);
        }
        Value::Loop {
            header,
            body,
            latch,
        } => {
            rewrite_block_names(header, name_remap);
            rewrite_block_names(body, name_remap);
            rewrite_block_names(latch, name_remap);
        }
        Value::Lambda { body, .. } => rewrite_block_names(body, name_remap),
        _ => {}
    }
}

fn lower_expr_block(ctx: &mut LowerCtx, expr: &HirExpr) -> (Block, Option<Local>) {
    let mut ops = vec![];
    let result = lower_expr(ctx, expr, &mut ops, true);
    (
        Block {
            params: vec![],
            ops,
            result,
        },
        result,
    )
}

fn lower_expr(
    ctx: &mut LowerCtx,
    expr: &HirExpr,
    ops: &mut Vec<Op>,
    pure_region: bool,
) -> Option<Local> {
    match expr {
        HirExpr::Int(n, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Int(*n),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Float(n, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Float(*n),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Bool(b, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Bool(*b),
                pure_region,
            });
            Some(l)
        }
        HirExpr::String(s, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::String(s.clone()),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Char(c, _) => {
            let l = ctx.fresh();
            ops.push(Op::Let {
                local: l,
                value: Value::Char(*c),
                pure_region,
            });
            Some(l)
        }
        HirExpr::Unit(_) => None,
        HirExpr::Var(name, _) => {
            if ctx.mutables.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Name(name.clone()),
                    pure_region,
                });
                Some(l)
            } else if let Some(l) = ctx.name_to_local.get(name) {
                Some(*l)
            } else if ctx.toplevel_funs.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::FunRef(name.clone()),
                    pure_region,
                });
                Some(l)
            } else if ctx.toplevel_vals.contains(name) {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Call {
                        fun: format!("__val_{name}"),
                        args: vec![],
                    },
                    pure_region,
                });
                Some(l)
            } else {
                let l = ctx.fresh();
                ops.push(Op::Let {
                    local: l,
                    value: Value::Name(name.clone()),
                    pure_region,
                });
                Some(l)
            }
        }
        HirExpr::Let {
            name,
            value,
            body,
            mutable,
            ..
        } => {
            let v = lower_expr(ctx, value, ops, pure_region);
            let saved = ctx.save_bindings();
            if let Some(l) = v {
                if *mutable {
                    ctx.bind_mutable(name.clone(), l);
                    ops.push(Op::Assign {
                        name: name.clone(),
                        value: l,
                    });
                } else {
                    // `val` may shadow an outer `var` for the duration of `body`.
                    ctx.mutables.remove(name);
                    ctx.bind_name(name.clone(), l);
                }
            }
            let result = lower_expr(ctx, body, ops, pure_region);
            ctx.restore_bindings(saved);
            result
        }
        HirExpr::Assign { name, value, .. } => {
            let v = match lower_expr(ctx, value, ops, pure_region) {
                Some(l) => l,
                None => {
                    // Unit RHS: materialize a 0 local so assign never panics.
                    let l = ctx.fresh();
                    ops.push(Op::Let {
                        local: l,
                        value: Value::Unit,
                        pure_region,
                    });
                    l
                }
            };
            if ctx.mutables.contains(name) {
                ops.push(Op::Assign {
                    name: name.clone(),
                    value: v,
                });
            } else {
                // Immutable binding: ty rejects user assigns; do not mutate an
                // outer `var` shadowed by `val` (and do not mark name mutable).
                ctx.bind_name(name.clone(), v);
            }
            None
        }
        HirExpr::Binary {
            op, left, right, ..
        } => {
            let l = lower_expr(ctx, left, ops, pure_region)
                .expect("ICE: binary operand lowered to Unit; type checker should reject");
            let r = lower_expr(ctx, right, ops, pure_region)
                .expect("ICE: binary operand lowered to Unit; type checker should reject");
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Binary {
                    op: *op,
                    left: l,
                    right: r,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Unary { op, expr, .. } => {
            let o = lower_expr(ctx, expr, ops, pure_region)
                .expect("ICE: unary operand lowered to Unit; type checker should reject");
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Unary {
                    op: *op,
                    operand: o,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Call { callee, args, .. } => {
            let mut arg_locals = vec![];
            for a in args {
                if let Some(l) = lower_expr(ctx, a, ops, pure_region) {
                    arg_locals.push(l);
                }
            }
            let dest = ctx.fresh();
            let fun_name = match callee.as_ref() {
                HirExpr::Var(n, _) => Some(n.as_str()),
                _ => None,
            };
            let value = match fun_name {
                Some("listOf") => Value::AllocList {
                    elems: arg_locals,
                    repr: ListRepr::HeapList,
                },
                Some("setOf") => Value::AllocSet {
                    elems: arg_locals,
                },
                Some("mapOf") => Value::AllocMap {
                    flat_pairs: arg_locals,
                    repr: MapRepr::HashOrdered,
                },
                Some(n) if ctx.toplevel_funs.contains(n) => Value::Call {
                    fun: n.to_string(),
                    args: arg_locals,
                },
                _ => {
                    // Local / expression callee → indirect call (first-class fn).
                    let cal = lower_expr(ctx, callee, ops, pure_region).unwrap_or_else(|| {
                        let l = ctx.fresh();
                        ops.push(Op::Let {
                            local: l,
                            value: Value::Int(0),
                            pure_region,
                        });
                        l
                    });
                    Value::IndirectCall {
                        callee: cal,
                        args: arg_locals,
                    }
                }
            };
            ops.push(Op::Let {
                local: dest,
                value,
                pure_region,
            });
            Some(dest)
        }
        HirExpr::BuiltinCall { name, args, .. } => {
            let mut arg_locals = vec![];
            // Product field checks carry an expected-ADT name as a 3rd HIR arg;
            // Core/runtime only need (obj, index).
            let use_args: &[HirExpr] = if matches!(name, Builtin::AdtField) && args.len() == 3 {
                &args[..2]
            } else {
                args
            };
            for a in use_args {
                if let Some(l) = lower_expr(ctx, a, ops, true) {
                    arg_locals.push(l);
                }
            }
            let is_io = matches!(
                name,
                Builtin::Println
                    | Builtin::PrintlnInt
                    | Builtin::PrintlnStr
                    | Builtin::ReadStdin
            );
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Builtin {
                    name: *name,
                    args: arg_locals,
                },
                pure_region: !is_io,
            });
            Some(dest)
        }
        HirExpr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let c = lower_expr(ctx, cond, ops, pure_region)
                .expect("ICE: if condition lowered to Unit; type checker should reject");
            // Isolate arm bindings so `val`/`var` inside then/else cannot leak.
            let saved = ctx.save_bindings();
            let (then_block, _) = lower_expr_block(ctx, then_branch);
            ctx.restore_bindings(saved.clone());
            let (else_block, _) = lower_expr_block(ctx, else_branch);
            ctx.restore_bindings(saved);
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::If {
                    cond: c,
                    then_block: Box::new(then_block),
                    else_block: Box::new(else_block),
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Loop {
            cond,
            body,
            step,
            ..
        } => {
            // Loop header/body/latch share outer bindings but must not leak
            // names introduced only inside those blocks.
            let saved = ctx.save_bindings();
            let (header, _) = lower_expr_block(ctx, cond);
            ctx.restore_bindings(saved.clone());
            let (body_block, _) = lower_expr_block(ctx, body);
            ctx.restore_bindings(saved.clone());
            let latch = if let Some(s) = step {
                let (b, _) = lower_expr_block(ctx, s);
                b
            } else {
                Block {
                    params: vec![],
                    ops: vec![],
                    result: None,
                }
            };
            ctx.restore_bindings(saved);
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Loop {
                    header: Box::new(header),
                    body: Box::new(body_block),
                    latch: Box::new(latch),
                },
                pure_region: false,
            });
            Some(dest)
        }
        HirExpr::Break(_) => {
            ops.push(Op::Break);
            None
        }
        HirExpr::Continue(_) => {
            ops.push(Op::Continue);
            None
        }
        HirExpr::AdtNew { tag, args, .. } => {
            let mut fields = vec![];
            for a in args {
                if let Some(l) = lower_expr(ctx, a, ops, pure_region) {
                    fields.push(l);
                }
            }
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::AllocAdt {
                    tag: *tag,
                    fields,
                },
                pure_region,
            });
            Some(dest)
        }
        HirExpr::Seq { stmts, .. } => {
            let mut last = None;
            for s in stmts {
                last = lower_expr(ctx, s, ops, pure_region);
            }
            last
        }
        HirExpr::Lambda { params, body, .. } => {
            let mut inner = LowerCtx {
                next: ctx.next,
                name_to_local: ctx.name_to_local.clone(),
                mutables: ctx.mutables.clone(),
                toplevel_funs: ctx.toplevel_funs.clone(),
                toplevel_vals: ctx.toplevel_vals.clone(),
            };
            let mut pls = vec![];
            for p in params {
                let l = inner.fresh();
                inner.bind_name(p.clone(), l);
                pls.push(l);
            }
            let (block, _) = lower_expr_block(&mut inner, body);
            ctx.next = inner.next;
            let dest = ctx.fresh();
            ops.push(Op::Let {
                local: dest,
                value: Value::Lambda {
                    params: pls,
                    body: Box::new(block),
                },
                pure_region,
            });
            Some(dest)
        }
    }
}

/// Display Core IR for `--show-ir`.
pub fn format_module(m: &CoreModule) -> String {
    let mut out = String::new();
    out.push_str(&format!("module {}\n", m.name));
    for f in &m.functions {
        out.push_str(&format!(
            "\nfun {}({}) effect.io={} memo={:?} {{\n",
            f.name,
            f.param_names.join(", "),
            f.effect.has_io(),
            f.memo
        ));
        format_block(&f.body, &mut out, 1);
        out.push_str("}\n");
    }
    out
}

fn format_block(b: &Block, out: &mut String, indent: usize) {
    let pad = "  ".repeat(indent);
    for op in &b.ops {
        match op {
            Op::Let {
                local,
                value,
                pure_region,
            } => {
                out.push_str(&format!(
                    "{pad}%{} = {}{}\n",
                    local.0,
                    format_value(value),
                    if *pure_region { "  // pure" } else { "  // effect" }
                ));
            }
            Op::Effect { value } => {
                out.push_str(&format!("{pad}effect {}\n", format_value(value)));
            }
            Op::Assign { name, value } => {
                out.push_str(&format!("{pad}{name} := %{}\n", value.0));
            }
            Op::Break => out.push_str(&format!("{pad}break\n")),
            Op::Continue => out.push_str(&format!("{pad}continue\n")),
        }
    }
    if let Some(r) = b.result {
        out.push_str(&format!("{pad}return %{}\n", r.0));
    }
}

fn format_value(v: &Value) -> String {
    match v {
        Value::Int(n) => format!("i64 {n}"),
        Value::Float(n) => format!("f64 {n}"),
        Value::Bool(b) => format!("bool {b}"),
        Value::String(s) => format!("str \"{s}\""),
        Value::Char(c) => format!("char {c:?}"),
        Value::Unit => "unit".into(),
        Value::Local(l) => format!("%{}", l.0),
        Value::Name(n) => format!("load {n}"),
        Value::Binary { op, left, right } => {
            format!("%{} {op} %{}", left.0, right.0)
        }
        Value::Unary { op, operand } => format!("{op:?} %{}", operand.0),
        Value::Call { fun, args } => {
            let a: Vec<_> = args.iter().map(|l| format!("%{}", l.0)).collect();
            format!("call {fun}({})", a.join(", "))
        }
        Value::IndirectCall { callee, args } => {
            let a: Vec<_> = args.iter().map(|l| format!("%{}", l.0)).collect();
            format!("icall %{}({})", callee.0, a.join(", "))
        }
        Value::Builtin { name, args } => {
            let a: Vec<_> = args.iter().map(|l| format!("%{}", l.0)).collect();
            format!("builtin {name:?}({})", a.join(", "))
        }
        Value::If { .. } => "if ...".into(),
        Value::Loop { .. } => "loop ...".into(),
        Value::Lambda { .. } => "lambda ...".into(),
        Value::FunRef(n) => format!("funref {n}"),
        Value::AllocClosure { fun, captures } => {
            format!("alloc_closure({fun}, n={})", captures.len())
        }
        Value::ClosureCap {
            env,
            index,
            as_float,
        } => {
            if *as_float {
                format!("closure_cap_f(%{}, {index})", env.0)
            } else {
                format!("closure_cap(%{}, {index})", env.0)
            }
        }
        Value::AllocList { elems, repr } => {
            format!("alloc_list[{repr:?}](n={})", elems.len())
        }
        Value::AllocSet { elems } => format!("alloc_set(n={})", elems.len()),
        Value::AllocMap { flat_pairs, repr } => {
            format!("alloc_map[{repr:?}](n={})", flat_pairs.len() / 2)
        }
        Value::AllocAdt { tag, fields } => {
            format!("alloc_adt(tag={tag}, n={})", fields.len())
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use lumia_hir::lower_module;
    use lumia_syntax::parse_module;
    use lumia_ty::infer_module;

    #[test]
    fn nested_identity_lambda_ret_ty_is_heap() {
        let src = r#"
module M
val main = {
    val id = { s -> s }
    id("hi")
}
"#;
        let ast = parse_module(src).unwrap();
        let hir = lower_module(&ast).expect("hir");
        let typed = infer_module(&hir).expect("ty");
        let core = lower_hir(&hir, &typed.fun_types);
        let lam = core
            .functions
            .iter()
            .find(|f| f.name.starts_with("__lam_"))
            .unwrap_or_else(|| panic!("expected lifted lambda, funs={:?}",
                core.functions.iter().map(|f| (&f.name, &f.ret_ty)).collect::<Vec<_>>()));
        assert!(
            !matches!(lam.ret_ty, Type::Int | Type::Bool | Type::Float | Type::Unit),
            "nested identity lambda must not claim scalar ret_ty (got {:?})",
            lam.ret_ty
        );
    }
}

//! List higher-order function desugaring (map/filter/fold/any/all/find).

use crate::ast::{Builtin, Expr};
use crate::lower::{counter_for_in, empty_list, for_each_elem, LowerCtx};
use crate::visit::free_vars_expr;
use lumia_syntax::{BinOp, Span};

/// Shared accumulate-over-list skeleton: `let mut acc = init; for x in list { step }; acc`.
fn list_accum(
    ctx: &LowerCtx,
    acc: String,
    init: Expr,
    x: &str,
    list: Expr,
    step: Expr,
    span: Span,
) -> Expr {
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                for_each_elem(ctx, x, list, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

/// Shared accumulate-over-range skeleton: `let mut acc = init; for x in start..end { step }; acc`.
fn range_accum(
    ctx: &LowerCtx,
    acc: String,
    init: Expr,
    x: &str,
    start: Expr,
    end: Expr,
    inclusive: bool,
    step: Expr,
    span: Span,
) -> Expr {
    Expr::Let {
        name: acc.clone(),
        value: Box::new(init),
        body: Box::new(Expr::Seq {
            stmts: vec![
                counter_for_in(ctx, x, start, end, inclusive, step, span),
                Expr::Var(acc, span),
            ],
            span,
        }),
        mutable: true,
    }
}

/// Optional `let f = … in body` when the callback is not a lambda.
fn with_fun_bind(f_bind: Option<(String, Expr)>, body: Expr) -> Expr {
    match f_bind {
        Some((name, val)) => Expr::Let {
            name,
            value: Box::new(val),
            body: Box::new(body),
            mutable: false,
        },
        None => body,
    }
}

/// Unary callback: inline lambda body vs bound function call.
enum UnaryCallback {
    Inline { param: String, body: Expr },
    Bound { f: Expr, f_name: String, x: String },
}

fn resolve_unary_callback(f: Expr, span: Span, prefix: &str) -> UnaryCallback {
    match f {
        Expr::Lambda { params, body, .. } if params.len() == 1 => UnaryCallback::Inline {
            param: params[0].clone(),
            body: *body,
        },
        f => {
            let f_name = format!("__{prefix}_f_{}", span.start.0);
            let x = format!("__{prefix}_x_{}", span.start.0);
            UnaryCallback::Bound { f, f_name, x }
        }
    }
}

/// Binary callback: inline lambda body vs bound function call.
enum BinaryCallback {
    Inline {
        acc: String,
        x: String,
        body: Expr,
    },
    Bound {
        f: Expr,
        f_name: String,
        acc: String,
        x: String,
    },
}

fn resolve_binary_callback(f: Expr, span: Span, prefix: &str) -> BinaryCallback {
    match f {
        Expr::Lambda { params, body, .. } if params.len() == 2 => BinaryCallback::Inline {
            acc: params[0].clone(),
            x: params[1].clone(),
            body: *body,
        },
        f => {
            let f_name = format!("__{prefix}_f_{}", span.start.0);
            let x = format!("__{prefix}_x_{}", span.start.0);
            let acc = format!("__{prefix}_acc_{}", span.start.0);
            BinaryCallback::Bound { f, f_name, acc, x }
        }
    }
}

/// `xs.map(f)` → `ListParMap` when FunRef-safe; else sequential accumulate.
/// Type checking may demote `ListParMap` back to sequential (IO / non-scalar).
pub(crate) fn lower_list_map(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    if map_callback_is_parallel_safe(ctx, &f) {
        return Expr::BuiltinCall {
            name: Builtin::ListParMap,
            args: vec![list, f],
            span,
        };
    }
    desugar_list_map_sequential(ctx, list, f, span)
}

/// Sequential `map` loop (also used when auto-parallel demotes `ListParMap`).
pub fn desugar_list_map_sequential(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    match resolve_unary_callback(f, span, "map") {
        UnaryCallback::Inline { param, body } => {
            lower_list_map_inline(ctx, list, param, body, span)
        }
        UnaryCallback::Bound { f, f_name, x } => lower_list_map_call(ctx, list, f, f_name, x, span),
    }
}

/// Parallel map: capture-free lambda, or a top-level function name (FunRef).
/// Free refs to other top-level funs (e.g. `{ x -> double(x) }`) are FunRef-safe.
fn map_callback_is_parallel_safe(ctx: &LowerCtx, f: &Expr) -> bool {
    match f {
        Expr::Lambda { params, body, .. } => {
            let bound: Vec<String> = params.clone();
            let frees = free_vars_expr(body, &bound);
            frees.iter().all(|n| ctx.is_toplevel_fun(n))
        }
        Expr::Var(n, _) => ctx.is_toplevel_fun(n),
        _ => false,
    }
}

/// `xs.sortBy(f)` — key must be Int / String / Char; stable permute of elements.
pub(crate) fn lower_list_sort_by(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    let xs = format!("__sby_xs_{}", span.start.0);
    let keys = format!("__sby_keys_{}", span.start.0);
    Expr::Let {
        name: xs.clone(),
        value: Box::new(list),
        body: Box::new(Expr::Let {
            name: keys.clone(),
            value: Box::new(lower_list_map(ctx, Expr::Var(xs.clone(), span), f, span)),
            body: Box::new(Expr::BuiltinCall {
                name: Builtin::ListSortByKeys,
                args: vec![Expr::Var(xs, span), Expr::Var(keys, span)],
                span,
            }),
            mutable: false,
        }),
        mutable: false,
    }
}

fn lower_list_map_inline(ctx: &LowerCtx, list: Expr, x: String, body: Expr, span: Span) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), body],
            span,
        }),
        span,
    };
    list_accum(ctx, acc, empty_list(span), &x, list, step, span)
}

fn lower_list_map_call(
    ctx: &LowerCtx,
    list: Expr,
    f: Expr,
    f_name: String,
    x: String,
    span: Span,
) -> Expr {
    let acc = format!("__map_acc_{}", span.start.0);
    let mapped = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(x.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), mapped],
            span,
        }),
        span,
    };
    with_fun_bind(
        Some((f_name, f)),
        list_accum(ctx, acc, empty_list(span), &x, list, step, span),
    )
}

pub(crate) fn lower_list_filter(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    match resolve_unary_callback(f, span, "flt") {
        UnaryCallback::Inline { param, body } => {
            lower_list_filter_inline(ctx, list, param, body, span)
        }
        UnaryCallback::Bound { f, f_name, x } => {
            lower_list_filter_call(ctx, list, f, f_name, x, span)
        }
    }
}

fn lower_list_filter_inline(ctx: &LowerCtx, list: Expr, x: String, body: Expr, span: Span) -> Expr {
    let acc = format!("__flt_acc_{}", span.start.0);
    let append = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    let step = Expr::If {
        cond: Box::new(body),
        then_branch: Box::new(append),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    list_accum(ctx, acc, empty_list(span), &x, list, step, span)
}

fn lower_list_filter_call(
    ctx: &LowerCtx,
    list: Expr,
    f: Expr,
    f_name: String,
    x: String,
    span: Span,
) -> Expr {
    let acc = format!("__flt_acc_{}", span.start.0);
    let pred = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(x.clone(), span)],
        span,
    };
    let append = Expr::Assign {
        name: acc.clone(),
        value: Box::new(Expr::BuiltinCall {
            name: Builtin::ListAppend,
            args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
            span,
        }),
        span,
    };
    let step = Expr::If {
        cond: Box::new(pred),
        then_branch: Box::new(append),
        else_branch: Box::new(Expr::Unit(span)),
        span,
    };
    with_fun_bind(
        Some((f_name, f)),
        list_accum(ctx, acc, empty_list(span), &x, list, step, span),
    )
}

fn apply_pred(f: &Expr, x: Expr, span: Span) -> Expr {
    match f {
        Expr::Lambda { params, body, .. } if params.len() == 1 => Expr::Let {
            name: params[0].clone(),
            value: Box::new(x),
            body: body.clone(),
            mutable: false,
        },
        _ => Expr::Call {
            callee: Box::new(f.clone()),
            args: vec![x],
            span,
        },
    }
}

/// `xs.flatMap(f)` where `f: T -> List[U]` → concat mapped lists.
pub(crate) fn lower_list_flat_map(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    let acc = format!("__fmap_acc_{}", span.start.0);
    match resolve_unary_callback(f, span, "fmap") {
        UnaryCallback::Inline { param, body } => {
            let x = format!("__fmap_x_{}", span.start.0);
            let mapped = Expr::Let {
                name: param,
                value: Box::new(Expr::Var(x.clone(), span)),
                body: Box::new(body),
                mutable: false,
            };
            let step = Expr::Assign {
                name: acc.clone(),
                value: Box::new(Expr::BuiltinCall {
                    name: Builtin::ListConcat,
                    args: vec![Expr::Var(acc.clone(), span), mapped],
                    span,
                }),
                span,
            };
            list_accum(ctx, acc, empty_list(span), &x, list, step, span)
        }
        UnaryCallback::Bound { f, f_name, x } => {
            let mapped = Expr::Call {
                callee: Box::new(Expr::Var(f_name.clone(), span)),
                args: vec![Expr::Var(x.clone(), span)],
                span,
            };
            let step = Expr::Assign {
                name: acc.clone(),
                value: Box::new(Expr::BuiltinCall {
                    name: Builtin::ListConcat,
                    args: vec![Expr::Var(acc.clone(), span), mapped],
                    span,
                }),
                span,
            };
            with_fun_bind(
                Some((f_name, f)),
                list_accum(ctx, acc, empty_list(span), &x, list, step, span),
            )
        }
    }
}

fn option_some(ctx: &LowerCtx, x: Expr, span: Span) -> Expr {
    match ctx.lookup_ctor("Some") {
        Some(c) => Expr::AdtNew {
            adt_name: c.adt_name,
            variant: "Some".into(),
            tag: c.tag,
            args: vec![x],
            span,
        },
        None => Expr::Call {
            callee: Box::new(Expr::Var("Some".into(), span)),
            args: vec![x],
            span,
        },
    }
}

fn option_none(ctx: &LowerCtx, span: Span) -> Expr {
    match ctx.lookup_ctor("None") {
        Some(c) => Expr::AdtNew {
            adt_name: c.adt_name,
            variant: "None".into(),
            tag: c.tag,
            args: vec![],
            span,
        },
        None => Expr::Call {
            callee: Box::new(Expr::Var("None".into(), span)),
            args: vec![],
            span,
        },
    }
}

/// Bind non-lambda `f` to a temp; lambdas stay inline.
fn bind_fun(f: Expr, span: Span) -> (Option<(String, Expr)>, Expr) {
    match &f {
        Expr::Lambda { .. } => (None, f),
        _ => {
            let name = format!("__pred_f_{}", span.start.0);
            (Some((name.clone(), f)), Expr::Var(name, span))
        }
    }
}

enum ListSearchKind {
    Any,
    All,
    Find,
}

/// Shared short-circuit search: `any` / `all` / `find`.
fn list_search(ctx: &LowerCtx, list: Expr, f: Expr, span: Span, kind: ListSearchKind) -> Expr {
    let (prefix, init) = match kind {
        ListSearchKind::Any => ("any", Expr::Bool(false, span)),
        ListSearchKind::All => ("all", Expr::Bool(true, span)),
        ListSearchKind::Find => ("find", option_none(ctx, span)),
    };
    let acc = format!("__{prefix}_acc_{}", span.start.0);
    let x = format!("__{prefix}_x_{}", span.start.0);
    let (f_bind, pred_f) = bind_fun(f, span);
    let pred = apply_pred(&pred_f, Expr::Var(x.clone(), span), span);
    let step = match kind {
        ListSearchKind::Any => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(Expr::Bool(true, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
        ListSearchKind::All => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Unit(span)),
            else_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(Expr::Bool(false, span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            span,
        },
        ListSearchKind::Find => Expr::If {
            cond: Box::new(pred),
            then_branch: Box::new(Expr::Seq {
                stmts: vec![
                    Expr::Assign {
                        name: acc.clone(),
                        value: Box::new(option_some(ctx, Expr::Var(x.clone(), span), span)),
                        span,
                    },
                    Expr::Break(span),
                ],
                span,
            }),
            else_branch: Box::new(Expr::Unit(span)),
            span,
        },
    };
    with_fun_bind(f_bind, list_accum(ctx, acc, init, &x, list, step, span))
}

pub(crate) fn lower_list_any(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    list_search(ctx, list, f, span, ListSearchKind::Any)
}

pub(crate) fn lower_list_all(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    list_search(ctx, list, f, span, ListSearchKind::All)
}

pub(crate) fn lower_list_find(ctx: &LowerCtx, list: Expr, f: Expr, span: Span) -> Expr {
    list_search(ctx, list, f, span, ListSearchKind::Find)
}

pub(crate) fn lower_list_fold(ctx: &LowerCtx, list: Expr, init: Expr, f: Expr, span: Span) -> Expr {
    // `range(a,b).fold(...)` → counter loop (no HeapList materialization).
    if let Expr::BuiltinCall { name, args, .. } = &list {
        if matches!(name, Builtin::Range | Builtin::RangeInclusive) && args.len() == 2 {
            let inclusive = matches!(name, Builtin::RangeInclusive);
            let start = args[0].clone();
            let end = args[1].clone();
            return match resolve_binary_callback(f, span, "fold") {
                BinaryCallback::Inline { acc, x, body } => {
                    range_fold_inline(ctx, start, end, inclusive, init, acc, x, body, span)
                }
                BinaryCallback::Bound { f, f_name, acc, x } => {
                    range_fold_call(ctx, start, end, inclusive, init, f, f_name, acc, x, span)
                }
            };
        }
    }
    // FunRef-safe list fold → parallel candidate (associativity assumed).
    if fold_callback_is_parallel_safe(ctx, &f) {
        return Expr::BuiltinCall {
            name: Builtin::ListParFold,
            args: vec![list, init, f],
            span,
        };
    }
    desugar_list_fold_sequential(ctx, list, init, f, span)
}

/// Sequential `fold` loop (also used when auto-parallel demotes `ListParFold`).
pub fn desugar_list_fold_sequential(
    ctx: &LowerCtx,
    list: Expr,
    init: Expr,
    f: Expr,
    span: Span,
) -> Expr {
    match resolve_binary_callback(f, span, "fold") {
        BinaryCallback::Inline { acc, x, body } => {
            lower_list_fold_inline(ctx, list, init, acc, x, body, span)
        }
        BinaryCallback::Bound { f, f_name, acc, x } => {
            lower_list_fold_call(ctx, list, init, f, f_name, acc, x, span)
        }
    }
}

/// Parallel fold: FunRef-safe **and** syntactically associative (`+` / `*`).
/// DESIGN: auto-parallel must not change values; non-associative ops (e.g. `-`)
/// yield wrong results under chunked combine.
fn fold_callback_is_parallel_safe(ctx: &LowerCtx, f: &Expr) -> bool {
    match f {
        Expr::Lambda { params, body, .. } if params.len() == 2 => {
            let bound: Vec<String> = params.clone();
            let frees = free_vars_expr(body, &bound);
            if !frees.iter().all(|n| ctx.is_toplevel_fun(n)) {
                return false;
            }
            fold_body_is_associative(body.as_ref(), &params[0], &params[1])
        }
        Expr::Var(n, _) => {
            if !ctx.is_toplevel_fun(n) {
                return false;
            }
            // Resolve top-level lambda body when registered as a val/fun binding.
            ctx.is_toplevel_fold_assoc(n)
        }
        _ => false,
    }
}

fn vars_are_fold_params(l: &str, r: &str, a: &str, b: &str) -> bool {
    (l == a && r == b) || (l == b && r == a)
}

trait FoldAssocExpr {
    fn match_add_mul_vars(&self, a: &str, b: &str) -> bool;
    fn nested_for_assoc(&self) -> Option<&Self>;
}

impl FoldAssocExpr for Expr {
    fn match_add_mul_vars(&self, a: &str, b: &str) -> bool {
        match self {
            Expr::Binary {
                op: BinOp::Add | BinOp::Mul,
                left,
                right,
                ..
            } => match (left.as_ref(), right.as_ref()) {
                (Expr::Var(l, _), Expr::Var(r, _)) => vars_are_fold_params(l, r, a, b),
                _ => false,
            },
            _ => false,
        }
    }

    fn nested_for_assoc(&self) -> Option<&Self> {
        match self {
            Expr::Seq { stmts, .. } => stmts.last(),
            Expr::Let { body, .. } => Some(body),
            _ => None,
        }
    }
}

impl FoldAssocExpr for lumia_syntax::Expr {
    fn match_add_mul_vars(&self, a: &str, b: &str) -> bool {
        match self {
            lumia_syntax::Expr::Binary {
                op: BinOp::Add | BinOp::Mul,
                left,
                right,
                ..
            } => match (left.as_ref(), right.as_ref()) {
                (lumia_syntax::Expr::Ident(l, _), lumia_syntax::Expr::Ident(r, _)) => {
                    vars_are_fold_params(l, r, a, b)
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn nested_for_assoc(&self) -> Option<&Self> {
        match self {
            lumia_syntax::Expr::Block { tail, .. } => tail.as_deref(),
            lumia_syntax::Expr::Lambda { body, .. } => Some(body),
            _ => None,
        }
    }
}

fn fold_body_is_associative<E: FoldAssocExpr>(body: &E, a: &str, b: &str) -> bool {
    if body.match_add_mul_vars(a, b) {
        return true;
    }
    body.nested_for_assoc()
        .is_some_and(|inner| fold_body_is_associative(inner, a, b))
}

/// Syntax-level twin of [`fold_body_is_associative`] (used while scanning items).
pub(crate) fn syntax_fold_body_is_associative(body: &lumia_syntax::Expr, a: &str, b: &str) -> bool {
    fold_body_is_associative(body, a, b)
}

fn range_fold_inline(
    ctx: &LowerCtx,
    start: Expr,
    end: Expr,
    inclusive: bool,
    init: Expr,
    acc: String,
    x: String,
    body: Expr,
    span: Span,
) -> Expr {
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(body),
        span,
    };
    range_accum(ctx, acc, init, &x, start, end, inclusive, step, span)
}

fn range_fold_call(
    ctx: &LowerCtx,
    start: Expr,
    end: Expr,
    inclusive: bool,
    init: Expr,
    f: Expr,
    f_name: String,
    acc: String,
    x: String,
    span: Span,
) -> Expr {
    let applied = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(applied),
        span,
    };
    with_fun_bind(
        Some((f_name, f)),
        range_accum(ctx, acc, init, &x, start, end, inclusive, step, span),
    )
}

fn lower_list_fold_inline(
    ctx: &LowerCtx,
    list: Expr,
    init: Expr,
    acc: String,
    x: String,
    body: Expr,
    span: Span,
) -> Expr {
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(body),
        span,
    };
    list_accum(ctx, acc, init, &x, list, step, span)
}

fn lower_list_fold_call(
    ctx: &LowerCtx,
    list: Expr,
    init: Expr,
    f: Expr,
    f_name: String,
    acc: String,
    x: String,
    span: Span,
) -> Expr {
    let applied = Expr::Call {
        callee: Box::new(Expr::Var(f_name.clone(), span)),
        args: vec![Expr::Var(acc.clone(), span), Expr::Var(x.clone(), span)],
        span,
    };
    let step = Expr::Assign {
        name: acc.clone(),
        value: Box::new(applied),
        span,
    };
    with_fun_bind(
        Some((f_name, f)),
        list_accum(ctx, acc, init, &x, list, step, span),
    )
}

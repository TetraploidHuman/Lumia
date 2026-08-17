//! Surface names, prelude ctors, and `Builtin::from_method` / display names.

use super::ast::Builtin;

impl Builtin {
    /// Resolve a surface method / free-function name + arity to a direct
    /// [`Builtin`] (no HOF desugar). Single table for HIR call lowering.
    pub fn from_method(name: &str, arity: usize) -> Option<Builtin> {
        use Builtin::*;
        Some(match (name, arity) {
            ("len", 1) => ListLen,
            ("get", 2) => ListGet,
            ("append", 2) => ListAppend,
            ("contains", 2) => Contains,
            ("set", 3) => MapSet,
            ("remove", 2) => MapRemove,
            ("insert", 2) => SetInsert,
            ("keys", 1) => MapKeys,
            ("values", 1) => MapValues,
            ("items", 1) => MapItems,
            ("slice", 2) | ("drop", 2) => ListSlice,
            ("take", 2) => ListTake,
            ("reverse", 1) => ListReverse,
            ("sort", 1) => ListSort,
            // `join` is overloaded (Task.join / List.join(sep)) — resolved in ty
            // from the receiver, not arity (arity alone false-greens / misdiagnoses).
            ("joinOpt", 1) => TaskJoinOpt,
            ("send", 2) => ChannelSend,
            ("recv", 1) => ChannelRecv,
            ("recvOpt", 1) => ChannelRecvOpt,
            ("close", 1) => ChannelClose,
            ("cancelScope", 0) => ScopeCancel,
            ("trim", 1) => StrTrim,
            ("split", 2) => StrSplit,
            ("substring", 3) => StrSubstring,
            ("toLower", 1) => StrToLower,
            ("toUpper", 1) => StrToUpper,
            ("startsWith", 2) => StrStartsWith,
            ("endsWith", 2) => StrEndsWith,
            ("readStdin", 0) => ReadStdin,
            ("concat", 2) => ListConcat,
            ("range", 2) => Range,
            ("rangeInclusive", 2) => RangeInclusive,
            _ => return None,
        })
    }

    /// Human-readable name for diagnostics.
    pub fn display_name(self) -> &'static str {
        use Builtin::*;
        match self {
            Println => "println",
            Show => "show",
            ReadStdin => "readStdin",
            MatchFail => "matchFail",
            Assert => "assert",
            ListLen => "len",
            ListGet => "get",
            ListSlice => "slice",
            ListAppend => "append",
            ListConcat => "concat",
            ListTake => "take",
            ListReverse => "reverse",
            ListSort => "sort",
            ListSortByKeys => "sortBy",
            ListParMap => "parMap",
            ListParFold => "parFold",
            ListJoin => "join",
            Elems => "elems",
            Range => "range",
            RangeInclusive => "rangeInclusive",
            Contains => "contains",
            MapSet => "set",
            MapRemove => "remove",
            SetInsert => "insert",
            MapKeys => "keys",
            MapValues => "values",
            MapItems => "items",
            StrTrim => "trim",
            StrSplit => "split",
            StrSubstring => "substring",
            StrToLower => "toLower",
            StrToUpper => "toUpper",
            StrStartsWith => "startsWith",
            StrEndsWith => "endsWith",
            AdtTag => "adtTag",
            AdtField => "adtField",
            ChannelNew => "channel",
            ChannelSend => "send",
            ChannelRecv => "recv",
            ChannelRecvOpt => "recvOpt",
            ChannelClose => "close",
            TaskJoin => "join",
            TaskJoinOpt => "joinOpt",
            TaskSpawn => "spawn",
            ScopeEnter => "scopeEnter",
            ScopeLeave => "scopeLeave",
            ScopeCancel => "cancelScope",
        }
    }

    /// Editor / docs role for this builtin, if it is a surface name at all.
    ///
    /// `None` hides compiler-internal ops (`adtTag`, `matchFail`, auto-par seeds).
    pub fn surface_role(self) -> Option<SurfaceRole> {
        use Builtin::*;
        match self {
            Println | Assert | ReadStdin | Range | RangeInclusive | ChannelNew | ScopeCancel => {
                Some(SurfaceRole::Free)
            }
            MatchFail | AdtTag | AdtField | ListParMap | ListParFold | TaskSpawn | ScopeEnter
            | ScopeLeave => None,
            _ => Some(SurfaceRole::Method),
        }
    }
}

/// How a surface name is typically written in source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// Free call: `listOf(…)`, `println(…)`, `range(…)`.
    Free,
    /// Dot / UFCS method: `xs.len()`, `xs.map(f)`.
    Method,
}

/// One completable / documentable surface identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceName {
    pub name: &'static str,
    pub role: SurfaceRole,
}

/// Collection constructors: **not** [`Builtin`] variants.
/// Typed in `lumia_ty::infer::prelude_ctors`, lowered to Core `AllocList` /
/// `AllocSet` / `AllocMap` (not via [`BuiltinInfo`] / runtime `lumia_*` symbols).
pub const PRELUDE_CTORS: &[SurfaceName] = &[
    SurfaceName {
        name: "listOf",
        role: SurfaceRole::Free,
    },
    SurfaceName {
        name: "mapOf",
        role: SurfaceRole::Free,
    },
    SurfaceName {
        name: "setOf",
        role: SurfaceRole::Free,
    },
];

/// Aliases accepted by [`Builtin::from_method`] that are not `display_name`.
const SURFACE_ALIASES: &[SurfaceName] = &[SurfaceName {
    name: "drop",
    role: SurfaceRole::Method,
}];

/// HOF / collection desugars in HIR lower (not a single [`Builtin`] call).
const HOF_SURFACE: &[SurfaceName] = &[
    SurfaceName {
        name: "map",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "filter",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "flatMap",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "fold",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "any",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "all",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "find",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "sortBy",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "isEmpty",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "toSet",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "toList",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "toMap",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "union",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "intersect",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "diff",
        role: SurfaceRole::Method,
    },
    SurfaceName {
        name: "lines",
        role: SurfaceRole::Method,
    },
];

/// All editor-facing names: prelude ctors + builtins + aliases + HOF desugars.
///
/// LSP completion / docs should scan this instead of maintaining a parallel list.
pub fn surface_names() -> impl Iterator<Item = SurfaceName> {
    PRELUDE_CTORS
        .iter()
        .copied()
        .chain(Builtin::ALL.iter().filter_map(|b| {
            b.surface_role().map(|role| SurfaceName {
                name: b.display_name(),
                role,
            })
        }))
        .chain(SURFACE_ALIASES.iter().copied())
        .chain(HOF_SURFACE.iter().copied())
}

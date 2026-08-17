//! Single source of truth for builtin arity / effects / runtime symbols / emit shape.

use super::ast::{Builtin, BuiltinFamily};

mod adt;
mod io;
mod list;
mod map_set;
mod string;
mod task;

#[cfg(test)]
mod tests;

pub(crate) use adt::info_adt;
pub(crate) use io::info_io;
pub(crate) use list::info_list;
pub(crate) use map_set::info_map_set;
pub(crate) use string::info_string;
pub(crate) use task::info_task;

/// Default effect for a builtin (actual call effect also unions argument effects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinEffect {
    Pure,
    Io,
}

/// Codegen calling convention for builtins that are a direct `lumia_*` call.
///
/// `Custom` stays hand-written (println/show/assert, FunRef checks for par_*).
/// Float container retagging uses [`BuiltinInfo::float_ensures`] on convention emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinEmit {
    Custom,
    /// `()` → heap ptr boxed as i64.
    NullaryPtr,
    /// `()` → void (unit).
    NullaryVoid,
    /// `(obj ptr)` → ptr→i64.
    UnaryObjPtr,
    /// `(obj ptr)` → scalar i64.
    UnaryObjScalar,
    /// `(obj ptr, i64)` → ptr→i64  (e.g. take/slice; **StrSplit** char as i64).
    ObjI64Ptr,
    /// `(obj ptr, i64)` → scalar.
    ObjI64Scalar,
    /// `(obj, obj)` → ptr→i64.
    ObjObjPtr,
    /// `(obj, obj)` → scalar.
    ObjObjScalar,
    /// `(i64, i64)` → ptr→i64.
    I64I64Ptr,
    /// `(obj ptr, i64, i64)` → ptr→i64.
    ObjI64I64Ptr,
    /// `(obj ptr, i64)` + codegen Option some/none tags → scalar i64 (`lumia_get`).
    ObjI64OptionTags,
}

/// Whether a builtin result may be a GC heap pointer (shadow-stack rooting).
///
/// Distinct from [`BuiltinInfo::may_capture`] (argument escape). Projections like
/// `ListGet` / `AdtField` do not capture args but may return heap values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultHeap {
    /// Result is never a heap pointer (Int/Bool/Unit / noreturn).
    Never,
    /// Result is always a heap object (List/Map/Set/String/…).
    Always,
    /// Depends on argument types — codegen uses `infer_value_ty` + `type_may_heap`.
    Typed,
}

/// Metadata driving ty arity checks and simple codegen symbol lookup.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinInfo {
    pub family: BuiltinFamily,
    pub min_arity: u8,
    pub max_arity: u8,
    pub effect: BuiltinEffect,
    /// Primary `lumia_*` runtime symbol when emission is a direct call.
    pub runtime_symbol: Option<&'static str>,
    /// When `args[arg_idx]` is Float, call `ensure_sym` on the container (`args[0]`)
    /// before the runtime call (List/Map/Set IEEE tagging).
    pub float_ensures: &'static [(u8, &'static str)],
    pub emit: BuiltinEmit,
    /// Escape analysis: whether arguments may be retained by the runtime
    /// (collections / IO). Pure projections (len/get/tag) are `false`.
    /// `Show` does not retain after return but is still `false` here — escape
    /// seeds Show operands separately so they are heap-rooted for `lumia_show`.
    pub may_capture: bool,
    /// Codegen GC rooting for the *result* (not args). See [`ResultHeap`].
    pub result_heap: ResultHeap,
    /// When the receiver is typed `String`, emit this RT symbol instead of
    /// [`Self::runtime_symbol`] (list-family methods overloaded on String).
    pub string_receiver_rt: Option<&'static str>,
    /// When the receiver is a known `List`, emit this monomorphic list RT symbol
    /// instead of the polymorphic map/container entry.
    pub list_receiver_rt: Option<&'static str>,
}

impl BuiltinInfo {
    pub fn float_sensitive(self) -> bool {
        !self.float_ensures.is_empty()
    }

    pub fn with_string_receiver_rt(mut self, sym: &'static str) -> Self {
        self.string_receiver_rt = Some(sym);
        self
    }

    pub fn with_list_receiver_rt(mut self, sym: &'static str) -> Self {
        self.list_receiver_rt = Some(sym);
        self
    }
}

pub(crate) const NO_F: &[(u8, &str)] = &[];
pub(crate) const ENS_LIST_APPEND: &[(u8, &str)] = &[(1, lumia_abi::ENSURE_LIST_F64)];
pub(crate) const ENS_SET_INSERT: &[(u8, &str)] = &[(1, lumia_abi::ENSURE_SET_F64)];
pub(crate) const ENS_MAP_SET: &[(u8, &str)] = &[
    (1, lumia_abi::ENSURE_MAP_F64),
    (2, lumia_abi::ENSURE_MAP_VF64),
];

#[inline]
pub(crate) fn bi(
    family: BuiltinFamily,
    min_arity: u8,
    max_arity: u8,
    effect: BuiltinEffect,
    runtime_symbol: Option<&'static str>,
    float_ensures: &'static [(u8, &'static str)],
    emit: BuiltinEmit,
    may_capture: bool,
    result_heap: ResultHeap,
) -> BuiltinInfo {
    BuiltinInfo {
        family,
        min_arity,
        max_arity,
        effect,
        runtime_symbol,
        float_ensures,
        emit,
        may_capture,
        result_heap,
        string_receiver_rt: None,
        list_receiver_rt: None,
    }
}

impl Builtin {
    /// Canonical metadata for this builtin.
    pub fn info(self) -> BuiltinInfo {
        use Builtin::*;
        match self {
            Println | Show | ReadStdin | MatchFail | Assert => info_io(self),
            ListLen | ListGet | ListSlice | ListAppend | ListConcat | ListTake | ListReverse
            | ListSort | ListSortByKeys | ListParMap | ListParFold | ListJoin | Elems | Range
            | RangeInclusive => info_list(self),
            Contains | MapSet | MapRemove | SetInsert | MapKeys | MapValues | MapItems => {
                info_map_set(self)
            }
            StrTrim | StrSplit | StrSubstring | StrToLower | StrToUpper | StrStartsWith
            | StrEndsWith => info_string(self),
            AdtTag | AdtField => info_adt(self),
            ChannelNew | ChannelSend | ChannelRecv | ChannelRecvOpt | ChannelClose | TaskJoin
            | TaskJoinOpt | TaskSpawn | ScopeEnter | ScopeLeave | ScopeCancel => info_task(self),
        }
    }

    /// Whether escape analysis should treat arguments as potentially captured.
    pub fn may_capture(self) -> bool {
        self.info().may_capture
    }

    /// How codegen should decide GC rooting for this builtin's result.
    pub fn result_heap(self) -> ResultHeap {
        self.info().result_heap
    }

    /// Whether this builtin may retag a Float container at the call site.
    pub fn float_sensitive(self) -> bool {
        self.info().float_sensitive()
    }

    /// Shared family used by `lumia_ty` and `lumia_codegen` routers.
    pub fn family(self) -> BuiltinFamily {
        self.info().family
    }

    /// Whether this builtin is effectful (`println` / `readStdin`).
    pub fn is_io(self) -> bool {
        matches!(self.info().effect, BuiltinEffect::Io)
    }

    /// Primary runtime symbol when emission is a direct `lumia_*` call.
    pub fn runtime_symbol(self) -> Option<&'static str> {
        self.info().runtime_symbol
    }

    /// When the receiver is typed `String`, use the dedicated String RT entry
    /// instead of the polymorphic / list symbol in [`Self::runtime_symbol`].
    pub fn string_receiver_rt_override(self) -> Option<&'static str> {
        self.info().string_receiver_rt
    }

    /// When the receiver is a known `List`, use the monomorphic list RT entry
    /// instead of the polymorphic map/container symbol.
    pub fn list_receiver_rt_override(self) -> Option<&'static str> {
        self.info().list_receiver_rt
    }

    /// Exhaustive list of builtins — keep in sync when adding a variant.
    pub const ALL: &[Builtin] = &[
        Builtin::Println,
        Builtin::ListLen,
        Builtin::ListGet,
        Builtin::ListSlice,
        Builtin::ListAppend,
        Builtin::ListConcat,
        Builtin::Contains,
        Builtin::MapSet,
        Builtin::MapRemove,
        Builtin::SetInsert,
        Builtin::MapKeys,
        Builtin::MapValues,
        Builtin::MapItems,
        Builtin::Elems,
        Builtin::Range,
        Builtin::RangeInclusive,
        Builtin::Show,
        Builtin::StrTrim,
        Builtin::StrSplit,
        Builtin::StrSubstring,
        Builtin::StrToLower,
        Builtin::StrToUpper,
        Builtin::StrStartsWith,
        Builtin::StrEndsWith,
        Builtin::ReadStdin,
        Builtin::MatchFail,
        Builtin::ListTake,
        Builtin::ListReverse,
        Builtin::ListSort,
        Builtin::ListSortByKeys,
        Builtin::ListParMap,
        Builtin::ListParFold,
        Builtin::Assert,
        Builtin::ListJoin,
        Builtin::AdtTag,
        Builtin::AdtField,
        Builtin::ChannelNew,
        Builtin::ChannelSend,
        Builtin::ChannelRecv,
        Builtin::ChannelRecvOpt,
        Builtin::ChannelClose,
        Builtin::TaskJoin,
        Builtin::TaskJoinOpt,
        Builtin::TaskSpawn,
        Builtin::ScopeEnter,
        Builtin::ScopeLeave,
        Builtin::ScopeCancel,
    ];
}

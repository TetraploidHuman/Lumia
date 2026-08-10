//! Single source of truth for builtin arity / effects / runtime symbols / emit shape.

use super::ast::{Builtin, BuiltinFamily};

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
}

impl BuiltinInfo {
    pub fn float_sensitive(self) -> bool {
        !self.float_ensures.is_empty()
    }
}

impl Builtin {
    /// Canonical metadata for this builtin.
    pub fn info(self) -> BuiltinInfo {
        use Builtin::*;
        use BuiltinEmit::*;
        let pure = BuiltinEffect::Pure;
        let io = BuiltinEffect::Io;
        const NO_F: &[(u8, &str)] = &[];
        const ENS_LIST_APPEND: &[(u8, &str)] = &[(1, lumia_abi::ENSURE_LIST_F64)];
        const ENS_SET_INSERT: &[(u8, &str)] = &[(1, lumia_abi::ENSURE_SET_F64)];
        const ENS_MAP_SET: &[(u8, &str)] = &[
            (1, lumia_abi::ENSURE_MAP_F64),
            (2, lumia_abi::ENSURE_MAP_VF64),
        ];
        let (family, min_arity, max_arity, effect, runtime_symbol, float_ensures, emit) = match self
        {
            Println => (BuiltinFamily::Io, 1, 1, io, None, NO_F, Custom),
            Show => (
                BuiltinFamily::Io,
                1,
                1,
                pure,
                Some("lumia_show"),
                NO_F,
                Custom,
            ),
            ReadStdin => (
                BuiltinFamily::Io,
                0,
                0,
                io,
                Some("lumia_read_stdin"),
                NO_F,
                NullaryPtr,
            ),
            MatchFail => (
                BuiltinFamily::Io,
                0,
                0,
                pure,
                Some("lumia_match_fail"),
                NO_F,
                NullaryVoid,
            ),
            // Annotated assert adds a message string → arity 1..=2.
            Assert => (
                BuiltinFamily::Io,
                1,
                2,
                pure,
                Some("lumia_assert"),
                NO_F,
                Custom,
            ),

            ListLen => (
                BuiltinFamily::List,
                1,
                1,
                pure,
                Some("lumia_len"),
                NO_F,
                UnaryObjScalar,
            ),
            ListGet => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_get"),
                NO_F,
                ObjI64OptionTags,
            ),
            ListSlice => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_slice"),
                NO_F,
                ObjI64Ptr,
            ),
            ListAppend => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_append"),
                ENS_LIST_APPEND,
                ObjI64Ptr,
            ),
            ListConcat => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_concat"),
                NO_F,
                ObjObjPtr,
            ),
            ListTake => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_take"),
                NO_F,
                ObjI64Ptr,
            ),
            ListReverse => (
                BuiltinFamily::List,
                1,
                1,
                pure,
                Some("lumia_list_reverse"),
                NO_F,
                UnaryObjPtr,
            ),
            ListSort => (
                BuiltinFamily::List,
                1,
                1,
                pure,
                Some("lumia_list_sort"),
                NO_F,
                UnaryObjPtr,
            ),
            ListSortByKeys => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_sort_by_keys"),
                NO_F,
                ObjObjPtr,
            ),
            ListParMap => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_par_map"),
                NO_F,
                Custom,
            ),
            ListParFold => (
                BuiltinFamily::List,
                3,
                3,
                pure,
                Some("lumia_list_par_fold"),
                NO_F,
                Custom,
            ),
            ListJoin => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_join"),
                NO_F,
                ObjObjPtr,
            ),
            Elems => (
                BuiltinFamily::List,
                1,
                1,
                pure,
                Some("lumia_elems"),
                NO_F,
                UnaryObjPtr,
            ),
            Range => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_range"),
                NO_F,
                I64I64Ptr,
            ),
            RangeInclusive => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_range_inclusive"),
                NO_F,
                I64I64Ptr,
            ),

            Contains => (
                BuiltinFamily::MapSet,
                2,
                2,
                pure,
                Some("lumia_contains"),
                NO_F,
                ObjI64Scalar,
            ),
            MapSet => (
                BuiltinFamily::MapSet,
                3,
                3,
                pure,
                Some("lumia_set"),
                ENS_MAP_SET,
                ObjI64I64Ptr,
            ),
            MapRemove => (
                BuiltinFamily::MapSet,
                2,
                2,
                pure,
                Some("lumia_remove"),
                NO_F,
                ObjI64Ptr,
            ),
            SetInsert => (
                BuiltinFamily::MapSet,
                2,
                2,
                pure,
                Some("lumia_set_insert"),
                ENS_SET_INSERT,
                ObjI64Ptr,
            ),
            MapKeys => (
                BuiltinFamily::MapSet,
                1,
                1,
                pure,
                Some("lumia_map_keys"),
                NO_F,
                UnaryObjPtr,
            ),
            MapValues => (
                BuiltinFamily::MapSet,
                1,
                1,
                pure,
                Some("lumia_map_values"),
                NO_F,
                UnaryObjPtr,
            ),
            MapItems => (
                BuiltinFamily::MapSet,
                1,
                1,
                pure,
                Some("lumia_map_items"),
                NO_F,
                UnaryObjPtr,
            ),

            StrTrim => (
                BuiltinFamily::String,
                1,
                1,
                pure,
                Some("lumia_str_trim"),
                NO_F,
                UnaryObjPtr,
            ),
            StrSplit => (
                BuiltinFamily::String,
                2,
                2,
                pure,
                Some("lumia_str_split"),
                NO_F,
                ObjI64Ptr,
            ),
            StrSubstring => (
                BuiltinFamily::String,
                3,
                3,
                pure,
                Some("lumia_str_substring"),
                NO_F,
                ObjI64I64Ptr,
            ),
            StrToLower => (
                BuiltinFamily::String,
                1,
                1,
                pure,
                Some("lumia_str_to_lower"),
                NO_F,
                UnaryObjPtr,
            ),
            StrToUpper => (
                BuiltinFamily::String,
                1,
                1,
                pure,
                Some("lumia_str_to_upper"),
                NO_F,
                UnaryObjPtr,
            ),
            StrStartsWith => (
                BuiltinFamily::String,
                2,
                2,
                pure,
                Some("lumia_str_starts_with"),
                NO_F,
                ObjObjScalar,
            ),
            StrEndsWith => (
                BuiltinFamily::String,
                2,
                2,
                pure,
                Some("lumia_str_ends_with"),
                NO_F,
                ObjObjScalar,
            ),

            AdtTag => (
                BuiltinFamily::Adt,
                1,
                1,
                pure,
                Some("lumia_adt_tag"),
                NO_F,
                UnaryObjScalar,
            ),
            AdtField => (
                // HIR may pass a 3rd name-hint arg; Core strips it before codegen
                // (`lower/expr/call.rs`) so emit stays `ObjI64Scalar` (2 args).
                BuiltinFamily::Adt,
                2,
                3,
                pure,
                Some("lumia_adt_field"),
                NO_F,
                ObjI64Scalar,
            ),
        };
        // Projections / traps that do not retain args for later use.
        let may_capture = !matches!(
            self,
            ListLen | ListGet | AdtTag | AdtField | Contains | Show | MatchFail
        );
        // Result GC rooting — orthogonal to may_capture (ListGet roots a String
        // element without retaining the list; ListParFold roots only if init heaps).
        let result_heap = match self {
            Println | ListLen | Contains | StrStartsWith | StrEndsWith | MatchFail | Assert
            | AdtTag => ResultHeap::Never,
            ListGet | AdtField | ListParFold => ResultHeap::Typed,
            _ => ResultHeap::Always,
        };
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
            ("join", 2) => ListJoin,
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
        }
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
    ];

    /// Editor / docs role for this builtin, if it is a surface name at all.
    ///
    /// `None` hides compiler-internal ops (`adtTag`, `matchFail`, auto-par seeds).
    pub fn surface_role(self) -> Option<SurfaceRole> {
        use Builtin::*;
        match self {
            Println | Assert | ReadStdin | Range | RangeInclusive => Some(SurfaceRole::Free),
            MatchFail | AdtTag | AdtField | ListParMap | ListParFold => None,
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

/// Collection constructors typed specially in `lumia_ty` (not [`Builtin`] variants).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_arity_allows_annotated_message() {
        let i = Builtin::Assert.info();
        assert_eq!(i.min_arity, 1);
        assert_eq!(i.max_arity, 2);
    }

    #[test]
    fn list_len_runtime_symbol() {
        assert_eq!(Builtin::ListLen.info().runtime_symbol, Some("lumia_len"));
    }

    #[test]
    fn str_split_is_obj_i64_not_obj_obj() {
        // Char separator must stay i64 — this is what broke ABI when helpers were wrong.
        assert_eq!(Builtin::StrSplit.info().emit, BuiltinEmit::ObjI64Ptr);
        assert_eq!(
            Builtin::StrStartsWith.info().emit,
            BuiltinEmit::ObjObjScalar
        );
    }

    #[test]
    fn from_method_covers_surface_aliases() {
        assert_eq!(Builtin::from_method("len", 1), Some(Builtin::ListLen));
        assert_eq!(Builtin::from_method("drop", 2), Some(Builtin::ListSlice));
        assert_eq!(Builtin::from_method("slice", 2), Some(Builtin::ListSlice));
        assert_eq!(Builtin::from_method("map", 2), None); // HOF desugar, not direct
    }

    #[test]
    fn may_capture_matches_escape_projection_set() {
        let no_capture = [
            Builtin::ListLen,
            Builtin::ListGet,
            Builtin::AdtTag,
            Builtin::AdtField,
            Builtin::Contains,
            Builtin::Show,
            Builtin::MatchFail,
        ];
        for b in no_capture {
            assert!(!b.may_capture(), "{}", b.display_name());
        }
        assert!(Builtin::ListAppend.may_capture());
        assert!(Builtin::MapSet.may_capture());
        assert!(Builtin::Println.may_capture());
        assert!(Builtin::ListParMap.may_capture());
    }

    #[test]
    fn every_builtin_has_coherent_info() {
        for &b in Builtin::ALL {
            let i = b.info();
            assert!(i.min_arity <= i.max_arity, "{}", b.display_name());
            if i.emit != BuiltinEmit::Custom {
                assert!(
                    i.runtime_symbol.is_some(),
                    "convention emit needs symbol: {}",
                    b.display_name()
                );
            }
            assert_eq!(i.family, b.family());
            assert_eq!(i.effect == BuiltinEffect::Io, b.is_io());
            assert_eq!(i.float_sensitive(), b.float_sensitive());
            assert_eq!(i.result_heap, b.result_heap());
            if i.float_sensitive() {
                assert!(
                    matches!(i.emit, BuiltinEmit::ObjI64Ptr | BuiltinEmit::ObjI64I64Ptr),
                    "float ensure should use convention emit: {}",
                    b.display_name()
                );
            }
            // Scalar convention emits that claim Always heap would over-root.
            if matches!(
                i.emit,
                BuiltinEmit::UnaryObjScalar | BuiltinEmit::ObjI64Scalar | BuiltinEmit::ObjObjScalar
            ) {
                assert_ne!(
                    i.result_heap,
                    ResultHeap::Always,
                    "scalar emit must not Always-root: {}",
                    b.display_name()
                );
            }
        }
    }

    #[test]
    fn result_heap_projections_are_typed_not_capture() {
        assert_eq!(Builtin::ListGet.result_heap(), ResultHeap::Typed);
        assert_eq!(Builtin::AdtField.result_heap(), ResultHeap::Typed);
        assert_eq!(Builtin::ListParFold.result_heap(), ResultHeap::Typed);
        assert!(!Builtin::ListGet.may_capture());
        assert!(!Builtin::AdtField.may_capture());
        assert!(Builtin::ListAppend.result_heap() == ResultHeap::Always);
        assert!(Builtin::ListLen.result_heap() == ResultHeap::Never);
        assert!(Builtin::Show.result_heap() == ResultHeap::Always);
        assert!(!Builtin::Show.may_capture());
    }

    #[test]
    fn surface_names_cover_prelude_and_common_methods() {
        let names: Vec<&str> = surface_names().map(|s| s.name).collect();
        for n in ["listOf", "setOf", "mapOf", "println", "len", "map", "drop"] {
            assert!(names.contains(&n), "missing surface name {n}");
        }
        assert!(!names.contains(&"adtTag"));
        assert!(!names.contains(&"matchFail"));
    }
}

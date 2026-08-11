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

const NO_F: &[(u8, &str)] = &[];
const ENS_LIST_APPEND: &[(u8, &str)] = &[(1, lumia_abi::ENSURE_LIST_F64)];
const ENS_SET_INSERT: &[(u8, &str)] = &[(1, lumia_abi::ENSURE_SET_F64)];
const ENS_MAP_SET: &[(u8, &str)] = &[
    (1, lumia_abi::ENSURE_MAP_F64),
    (2, lumia_abi::ENSURE_MAP_VF64),
];

#[inline]
fn bi(
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
    }
}

fn info_io(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let io = BuiltinEffect::Io;
    let f = BuiltinFamily::Io;
    match b {
        // Show: may_capture false; escape seed treats Show specially for rooting.
        Println => bi(f, 1, 1, io, None, NO_F, Custom, true, Never),
        Show => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_show"),
            NO_F,
            Custom,
            false,
            Always,
        ),
        ReadStdin => bi(
            f,
            0,
            0,
            io,
            Some("lumia_read_stdin"),
            NO_F,
            NullaryPtr,
            true,
            Always,
        ),
        MatchFail => bi(
            f,
            0,
            0,
            pure,
            Some("lumia_match_fail"),
            NO_F,
            NullaryVoid,
            false,
            Never,
        ),
        Assert => bi(
            f,
            1,
            2,
            pure,
            Some("lumia_assert"),
            NO_F,
            Custom,
            false,
            Never,
        ),
        _ => unreachable!("info_io: {b:?}"),
    }
}

fn info_list(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let f = BuiltinFamily::List;
    match b {
        ListLen => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_len"),
            NO_F,
            UnaryObjScalar,
            false,
            Never,
        ),
        ListGet => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_get"),
            NO_F,
            ObjI64OptionTags,
            false,
            Typed,
        ),
        ListSlice => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_list_slice"),
            NO_F,
            ObjI64Ptr,
            true,
            Always,
        ),
        ListAppend => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_list_append"),
            ENS_LIST_APPEND,
            ObjI64Ptr,
            true,
            Always,
        ),
        ListConcat => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_concat"),
            NO_F,
            ObjObjPtr,
            true,
            Always,
        ),
        ListTake => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_list_take"),
            NO_F,
            ObjI64Ptr,
            true,
            Always,
        ),
        ListReverse => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_list_reverse"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        ListSort => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_list_sort"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        ListSortByKeys => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_list_sort_by_keys"),
            NO_F,
            ObjObjPtr,
            true,
            Always,
        ),
        ListParMap => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_list_par_map"),
            NO_F,
            Custom,
            true,
            Always,
        ),
        ListParFold => bi(
            f,
            3,
            3,
            pure,
            Some("lumia_list_par_fold"),
            NO_F,
            Custom,
            true,
            Typed,
        ),
        ListJoin => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_list_join"),
            NO_F,
            ObjObjPtr,
            false,
            Always,
        ),
        Elems => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_elems"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        Range => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_range"),
            NO_F,
            I64I64Ptr,
            true,
            Always,
        ),
        RangeInclusive => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_range_inclusive"),
            NO_F,
            I64I64Ptr,
            true,
            Always,
        ),
        _ => unreachable!("info_list: {b:?}"),
    }
}

fn info_map_set(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let f = BuiltinFamily::MapSet;
    match b {
        Contains => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_contains"),
            NO_F,
            ObjI64Scalar,
            false,
            Never,
        ),
        MapSet => bi(
            f,
            3,
            3,
            pure,
            Some("lumia_set"),
            ENS_MAP_SET,
            ObjI64I64Ptr,
            true,
            Always,
        ),
        MapRemove => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_remove"),
            NO_F,
            ObjI64Ptr,
            true,
            Always,
        ),
        SetInsert => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_set_insert"),
            ENS_SET_INSERT,
            ObjI64Ptr,
            true,
            Always,
        ),
        MapKeys => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_map_keys"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        MapValues => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_map_values"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        MapItems => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_map_items"),
            NO_F,
            UnaryObjPtr,
            true,
            Always,
        ),
        _ => unreachable!("info_map_set: {b:?}"),
    }
}

fn info_string(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let f = BuiltinFamily::String;
    match b {
        StrTrim => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_str_trim"),
            NO_F,
            UnaryObjPtr,
            false,
            Always,
        ),
        StrSplit => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_str_split"),
            NO_F,
            ObjI64Ptr,
            false,
            Always,
        ),
        StrSubstring => bi(
            f,
            3,
            3,
            pure,
            Some("lumia_str_substring"),
            NO_F,
            ObjI64I64Ptr,
            false,
            Always,
        ),
        StrToLower => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_str_to_lower"),
            NO_F,
            UnaryObjPtr,
            false,
            Always,
        ),
        StrToUpper => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_str_to_upper"),
            NO_F,
            UnaryObjPtr,
            false,
            Always,
        ),
        StrStartsWith => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_str_starts_with"),
            NO_F,
            ObjObjScalar,
            false,
            Never,
        ),
        StrEndsWith => bi(
            f,
            2,
            2,
            pure,
            Some("lumia_str_ends_with"),
            NO_F,
            ObjObjScalar,
            false,
            Never,
        ),
        _ => unreachable!("info_string: {b:?}"),
    }
}

fn info_adt(b: Builtin) -> BuiltinInfo {
    use Builtin::*;
    use BuiltinEmit::*;
    use ResultHeap::*;
    let pure = BuiltinEffect::Pure;
    let f = BuiltinFamily::Adt;
    match b {
        AdtTag => bi(
            f,
            1,
            1,
            pure,
            Some("lumia_adt_tag"),
            NO_F,
            UnaryObjScalar,
            false,
            Never,
        ),
        // HIR may pass a 3rd name-hint arg; Core strips it before codegen.
        AdtField => bi(
            f,
            2,
            3,
            pure,
            Some("lumia_adt_field"),
            NO_F,
            ObjI64Scalar,
            false,
            Typed,
        ),
        _ => unreachable!("info_adt: {b:?}"),
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
        // Full no-capture set must stay in sync with `info()` — do not hand-curate a subset.
        let no_capture: Vec<_> = Builtin::ALL
            .iter()
            .copied()
            .filter(|b| !b.may_capture())
            .collect();
        let expected_no_capture = [
            Builtin::ListLen,
            Builtin::ListGet,
            Builtin::AdtTag,
            Builtin::AdtField,
            Builtin::Contains,
            Builtin::Show,
            Builtin::MatchFail,
            Builtin::Assert,
            Builtin::ListJoin,
            Builtin::StrTrim,
            Builtin::StrToLower,
            Builtin::StrToUpper,
            Builtin::StrSubstring,
            Builtin::StrSplit,
            Builtin::StrStartsWith,
            Builtin::StrEndsWith,
        ];
        assert_eq!(
            no_capture.len(),
            expected_no_capture.len(),
            "no-capture set drifted: got {:?}",
            no_capture
                .iter()
                .map(|b| b.display_name())
                .collect::<Vec<_>>()
        );
        for b in expected_no_capture {
            assert!(
                no_capture.contains(&b),
                "missing from no-capture: {}",
                b.display_name()
            );
            assert!(!b.may_capture(), "{}", b.display_name());
        }
        // Never-heap scalar probes must not capture (same class as Contains).
        for b in [
            Builtin::Contains,
            Builtin::StrStartsWith,
            Builtin::StrEndsWith,
            Builtin::ListLen,
            Builtin::AdtTag,
            Builtin::Assert,
            Builtin::MatchFail,
        ] {
            assert_eq!(b.result_heap(), ResultHeap::Never, "{}", b.display_name());
            assert!(!b.may_capture(), "{}", b.display_name());
        }
        // Identity / element-pointer copiers must capture.
        assert!(Builtin::Elems.may_capture());
        assert!(Builtin::MapItems.may_capture());
        assert!(Builtin::ListTake.may_capture());
        assert!(Builtin::ListSlice.may_capture());
        assert!(Builtin::ListReverse.may_capture());
        assert!(Builtin::ListSort.may_capture());
        assert!(Builtin::MapKeys.may_capture());
        assert!(Builtin::MapValues.may_capture());
        assert!(Builtin::ListAppend.may_capture());
        assert!(Builtin::ListConcat.may_capture());
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
    fn surface_from_method_roundtrips_display_name() {
        // Builtins reachable via method/free surface (not HOF/internal-only).
        let surface = [
            Builtin::ListLen,
            Builtin::ListGet,
            Builtin::ListAppend,
            Builtin::Contains,
            Builtin::MapSet,
            Builtin::MapRemove,
            Builtin::SetInsert,
            Builtin::MapKeys,
            Builtin::MapValues,
            Builtin::MapItems,
            Builtin::ListSlice,
            Builtin::ListTake,
            Builtin::ListReverse,
            Builtin::ListSort,
            Builtin::ListJoin,
            Builtin::StrTrim,
            Builtin::StrSplit,
            Builtin::StrSubstring,
            Builtin::StrToLower,
            Builtin::StrToUpper,
            Builtin::StrStartsWith,
            Builtin::StrEndsWith,
            Builtin::ReadStdin,
            Builtin::ListConcat,
            Builtin::Range,
            Builtin::RangeInclusive,
        ];
        for b in surface {
            let name = b.display_name();
            let arity = b.info().min_arity as usize;
            assert_eq!(
                Builtin::from_method(name, arity),
                Some(b),
                "from_method({name:?}, {arity}) should yield {b:?}"
            );
        }
        // Alias kept in from_method only.
        assert_eq!(Builtin::from_method("drop", 2), Some(Builtin::ListSlice));
        // Internal / HOF builtins stay out of from_method.
        for b in [
            Builtin::Println,
            Builtin::Show,
            Builtin::MatchFail,
            Builtin::Assert,
            Builtin::ListParMap,
            Builtin::ListParFold,
            Builtin::ListSortByKeys,
            Builtin::Elems,
            Builtin::AdtTag,
            Builtin::AdtField,
        ] {
            assert_eq!(
                Builtin::from_method(b.display_name(), b.info().min_arity as usize),
                None,
                "{} must not be a direct from_method surface",
                b.display_name()
            );
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
        let names: Vec<&str> = crate::surface_names().map(|s| s.name).collect();
        for n in ["listOf", "setOf", "mapOf", "println", "len", "map", "drop"] {
            assert!(names.contains(&n), "missing surface name {n}");
        }
        assert!(!names.contains(&"adtTag"));
        assert!(!names.contains(&"matchFail"));
    }
}

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
/// `Custom` stays hand-written (println/show/assert, Option tags, FunRef checks).
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
                Custom,
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
        BuiltinInfo {
            family,
            min_arity,
            max_arity,
            effect,
            runtime_symbol,
            float_ensures,
            emit,
        }
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
    fn every_builtin_has_coherent_info() {
        // Exhaustive mirror of `Builtin` — adding a variant must update this list
        // and `Builtin::info`.
        let all = [
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
        for b in all {
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
            if i.float_sensitive() {
                assert!(
                    matches!(i.emit, BuiltinEmit::ObjI64Ptr | BuiltinEmit::ObjI64I64Ptr),
                    "float ensure should use convention emit: {}",
                    b.display_name()
                );
            }
        }
    }
}

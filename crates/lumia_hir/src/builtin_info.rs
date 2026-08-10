//! Single source of truth for builtin arity / effects / runtime symbols.

use super::ast::{Builtin, BuiltinFamily};

/// Default effect for a builtin (actual call effect also unions argument effects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinEffect {
    Pure,
    Io,
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
    /// Container / float tagging may be needed at the call site.
    pub float_sensitive: bool,
}

impl Builtin {
    /// Canonical metadata for this builtin.
    pub fn info(self) -> BuiltinInfo {
        use Builtin::*;
        let pure = BuiltinEffect::Pure;
        let io = BuiltinEffect::Io;
        let (family, min_arity, max_arity, effect, runtime_symbol, float_sensitive) = match self {
            Println | PrintlnInt | PrintlnStr => (BuiltinFamily::Io, 1, 1, io, None, false),
            Show => (BuiltinFamily::Io, 1, 1, pure, Some("lumia_show"), false),
            ReadStdin => (BuiltinFamily::Io, 0, 0, io, Some("lumia_read_stdin"), false),
            MatchFail => (
                BuiltinFamily::Io,
                0,
                0,
                pure,
                Some("lumia_match_fail"),
                false,
            ),
            // Annotated assert adds a message string → arity 1..=2.
            Assert => (BuiltinFamily::Io, 1, 2, pure, Some("lumia_assert"), false),

            ListLen => (BuiltinFamily::List, 1, 1, pure, Some("lumia_len"), false),
            ListGet => (BuiltinFamily::List, 2, 2, pure, Some("lumia_get"), false),
            // `xs.slice(i)` / `xs.drop(n)` / match rest → (list, start); end is implicit.
            ListSlice => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_slice"),
                false,
            ),
            ListAppend => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_append"),
                true,
            ),
            ListConcat => (BuiltinFamily::List, 2, 2, pure, Some("lumia_concat"), false),
            ListTake => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_take"),
                false,
            ),
            ListReverse => (
                BuiltinFamily::List,
                1,
                1,
                pure,
                Some("lumia_list_reverse"),
                false,
            ),
            ListSort => (
                BuiltinFamily::List,
                1,
                1,
                pure,
                Some("lumia_list_sort"),
                false,
            ),
            ListSortByKeys => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_sort_by_keys"),
                false,
            ),
            ListParMap => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_par_map"),
                true,
            ),
            ListParFold => (
                BuiltinFamily::List,
                3,
                3,
                pure,
                Some("lumia_list_par_fold"),
                false,
            ),
            ListJoin => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_list_join"),
                false,
            ),
            Elems => (BuiltinFamily::List, 1, 1, pure, Some("lumia_elems"), false),
            Range => (BuiltinFamily::List, 2, 2, pure, Some("lumia_range"), false),
            RangeInclusive => (
                BuiltinFamily::List,
                2,
                2,
                pure,
                Some("lumia_range_inclusive"),
                false,
            ),

            Contains => (
                BuiltinFamily::MapSet,
                2,
                2,
                pure,
                Some("lumia_contains"),
                false,
            ),
            MapSet => (BuiltinFamily::MapSet, 3, 3, pure, Some("lumia_set"), true),
            MapRemove => (
                BuiltinFamily::MapSet,
                2,
                2,
                pure,
                Some("lumia_remove"),
                false,
            ),
            SetInsert => (
                BuiltinFamily::MapSet,
                2,
                2,
                pure,
                Some("lumia_set_insert"),
                true,
            ),
            MapKeys => (
                BuiltinFamily::MapSet,
                1,
                1,
                pure,
                Some("lumia_map_keys"),
                false,
            ),
            MapValues => (
                BuiltinFamily::MapSet,
                1,
                1,
                pure,
                Some("lumia_map_values"),
                false,
            ),
            MapItems => (
                BuiltinFamily::MapSet,
                1,
                1,
                pure,
                Some("lumia_map_items"),
                false,
            ),

            StrTrim => (
                BuiltinFamily::String,
                1,
                1,
                pure,
                Some("lumia_str_trim"),
                false,
            ),
            StrSplit => (
                BuiltinFamily::String,
                2,
                2,
                pure,
                Some("lumia_str_split"),
                false,
            ),
            StrSubstring => (
                BuiltinFamily::String,
                3,
                3,
                pure,
                Some("lumia_str_substring"),
                false,
            ),
            StrToLower => (
                BuiltinFamily::String,
                1,
                1,
                pure,
                Some("lumia_str_to_lower"),
                false,
            ),
            StrToUpper => (
                BuiltinFamily::String,
                1,
                1,
                pure,
                Some("lumia_str_to_upper"),
                false,
            ),
            StrStartsWith => (
                BuiltinFamily::String,
                2,
                2,
                pure,
                Some("lumia_str_starts_with"),
                false,
            ),
            StrEndsWith => (
                BuiltinFamily::String,
                2,
                2,
                pure,
                Some("lumia_str_ends_with"),
                false,
            ),

            AdtTag => (BuiltinFamily::Adt, 1, 1, pure, Some("lumia_adt_tag"), false),
            AdtField => (
                BuiltinFamily::Adt,
                2,
                3,
                pure,
                Some("lumia_adt_field"),
                false,
            ),
        };
        BuiltinInfo {
            family,
            min_arity,
            max_arity,
            effect,
            runtime_symbol,
            float_sensitive,
        }
    }

    /// Shared family used by `lumia_ty` and `lumia_codegen` routers.
    pub fn family(self) -> BuiltinFamily {
        self.info().family
    }

    /// Human-readable name for diagnostics.
    pub fn display_name(self) -> &'static str {
        use Builtin::*;
        match self {
            Println | PrintlnInt | PrintlnStr => "println",
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
}

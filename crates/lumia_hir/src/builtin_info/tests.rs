use super::{BuiltinEffect, BuiltinEmit, ResultHeap};
use crate::ast::Builtin;

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
        Builtin::ListTake,
        Builtin::ListSlice,
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
        Builtin::ChannelRecv,
        Builtin::ChannelRecvOpt,
        Builtin::ChannelClose,
        Builtin::TaskJoin,
        Builtin::TaskJoinOpt,
        Builtin::ScopeEnter,
        Builtin::ScopeLeave,
        Builtin::ScopeCancel,
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
    // Identity / element-pointer copiers must capture at seed (always).
    // Take/Slice escape only when their result escapes (propagate).
    assert!(Builtin::Elems.may_capture());
    assert!(Builtin::MapItems.may_capture());
    assert!(!Builtin::ListTake.may_capture());
    assert!(!Builtin::ListSlice.may_capture());
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
        Builtin::ChannelSend,
        Builtin::ChannelRecv,
        Builtin::ChannelRecvOpt,
        Builtin::ChannelClose,
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
        Builtin::ChannelNew,
        Builtin::TaskSpawn,
        Builtin::TaskJoin,
        Builtin::ListJoin,
        Builtin::ScopeEnter,
        Builtin::ScopeLeave,
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
fn receiver_rt_overrides_are_complete() {
    assert_eq!(
        Builtin::ListReverse.string_receiver_rt_override(),
        Some("lumia_str_reverse")
    );
    assert_eq!(
        Builtin::ListTake.string_receiver_rt_override(),
        Some("lumia_str_take")
    );
    assert_eq!(
        Builtin::ListSlice.string_receiver_rt_override(),
        Some("lumia_str_slice")
    );
    assert_eq!(
        Builtin::ListConcat.string_receiver_rt_override(),
        Some("lumia_str_concat")
    );
    assert_eq!(Builtin::ListAppend.string_receiver_rt_override(), None);
    assert_eq!(Builtin::ListLen.string_receiver_rt_override(), None);

    assert_eq!(
        Builtin::ListLen.list_receiver_rt_override(),
        Some("lumia_list_len")
    );
    assert_eq!(
        Builtin::MapSet.list_receiver_rt_override(),
        Some("lumia_list_set")
    );
    assert_eq!(
        Builtin::ListGet.list_receiver_rt_override(),
        Some("lumia_list_get")
    );
    assert_eq!(Builtin::ListConcat.list_receiver_rt_override(), None);
}

#[test]
fn surface_names_cover_prelude_and_common_methods() {
    let names: Vec<&str> = crate::surface_names().map(|s| s.name).collect();
    for n in [
        "listOf",
        "setOf",
        "mapOf",
        "println",
        "channel",
        "len",
        "map",
        "drop",
        "send",
        "recv",
        "join",
        "joinOpt",
        "cancelScope",
    ] {
        assert!(names.contains(&n), "missing surface name {n}");
    }
    assert!(!names.contains(&"adtTag"));
    assert!(!names.contains(&"matchFail"));
}

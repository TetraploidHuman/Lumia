use super::{for_each_expr, lower_module, Builtin, BuiltinFamily, Expr, Item};
use lumia_syntax::parse_module;

#[test]
fn builtin_family_routes_map_keys_with_map_set() {
    assert_eq!(Builtin::MapKeys.family(), BuiltinFamily::MapSet);
    assert_eq!(Builtin::Elems.family(), BuiltinFamily::List);
    assert_eq!(Builtin::ListLen.family(), BuiltinFamily::List);
    assert_eq!(Builtin::Show.family(), BuiltinFamily::Io);
    assert_eq!(Builtin::ChannelNew.family(), BuiltinFamily::Task);
    assert_eq!(Builtin::TaskJoin.family(), BuiltinFamily::Task);
}

#[test]
fn builtin_effect_and_symbols_are_wired() {
    assert!(Builtin::Println.is_io());
    assert!(Builtin::ReadStdin.is_io());
    assert!(!Builtin::ListLen.is_io());
    assert!(!Builtin::Assert.is_io());
    assert_eq!(Builtin::ListLen.runtime_symbol(), Some("lumia_len"));
    assert_eq!(Builtin::Println.runtime_symbol(), None);
    assert!(Builtin::ListAppend.info().float_sensitive());
    assert!(!Builtin::ListLen.info().float_sensitive());
    assert_eq!(
        Builtin::ListAppend.info().float_ensures,
        &[(1, lumia_abi::ENSURE_LIST_F64)]
    );
    assert_eq!(
        Builtin::ListAppend.info().bool_ensures,
        &[(1, lumia_abi::ENSURE_LIST_BOOL)]
    );
    assert_eq!(
        Builtin::MapSet.info().bool_ensures,
        &[
            (1, lumia_abi::ENSURE_MAP_BOOL),
            (2, lumia_abi::ENSURE_MAP_VBOOL)
        ]
    );
    assert_eq!(
        Builtin::MapSet.info().emit,
        super::BuiltinEmit::ObjI64I64Ptr
    );
    assert_eq!(
        Builtin::MapItems.info().emit,
        super::BuiltinEmit::UnaryObjBoolMask
    );
    assert_eq!(Builtin::MapItems.runtime_symbol(), Some("lumia_map_items"));
    assert_eq!(
        Builtin::SetInsert.info().emit,
        super::BuiltinEmit::ObjI64Ptr
    );
    assert_eq!(Builtin::StrSplit.info().emit, super::BuiltinEmit::ObjI64Ptr);
    assert_eq!(
        Builtin::ListGet.info().emit,
        super::BuiltinEmit::ObjI64OptionTags
    );
}

#[test]
fn method_surface_lowers_to_builtin_calls() {
    let src = r#"
module M
val main = {
listOf(1, 2).len()
listOf(1, 2).drop(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut saw_len = false;
    let mut saw_slice = false;
    crate::visit::for_each_expr(body, &mut |e| {
        if let Expr::BuiltinCall { name, .. } = e {
            saw_len |= *name == Builtin::ListLen;
            saw_slice |= *name == Builtin::ListSlice;
        }
    });
    assert!(saw_len, "expected ListLen from .len()");
    assert!(saw_slice, "expected ListSlice from .drop()");
}

#[test]
fn exhaustiveness_rejects_missing_variant() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Some"), "{err}");
}

#[test]
fn exhaustiveness_accepts_full_option() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(n) -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_nested_missing() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(None) -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Some"), "{err}");
    assert!(err.contains("in Some"), "{err}");
}

#[test]
fn exhaustiveness_accepts_nested_option() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(None) -> { 1 }
    Some(Some(n)) -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_nested_result_missing_err() {
    let src = r#"
module M
type Option { Some(value) None }
type Result { Ok(value) Err(msg) }
val f = { o ->
o match {
    None -> { 0 }
    Some(Ok(n)) -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Err"), "{err}");
}

#[test]
fn exhaustiveness_rejects_product_field_gap() {
    let src = r#"
module M
type Option { Some(value) None }
type Box { val inner }
val f = { b ->
b match {
    Box { inner = Some(n) } -> { n }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("None"), "{err}");
}

#[test]
fn exhaustiveness_accepts_nested_catch_all_payload() {
    let src = r#"
module M
type Option { Some(value) None }
type Result { Ok(value) Err(msg) }
val f = { o ->
o match {
    None -> { 0 }
    Some(_) -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_int_literals_without_wildcard() {
    let src = r#"
module M
val f = { n ->
n match {
    0 -> { 1 }
    1 -> { 2 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Int"), "{err}");
}

#[test]
fn exhaustiveness_rejects_empty_match() {
    let src = r#"
module M
val f = { n ->
n match { }
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn exhaustiveness_rejects_guard_only_arms() {
    let src = r#"
module M
val f = { n ->
n match {
    x if x > 0 -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn exhaustiveness_accepts_int_with_wildcard() {
    let src = r#"
module M
val f = { n ->
n match {
    0 -> { 1 }
    _ -> { 2 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_accepts_bool_both_arms() {
    let src = r#"
module M
val f = { b ->
b match {
    true -> { 1 }
    false -> { 0 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok(), "{:?}", lower_module(&ast));
}

#[test]
fn exhaustiveness_rejects_bool_missing_false() {
    let src = r#"
module M
val f = { b ->
b match {
    true -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("false") || err.contains("Bool"), "{err}");
}

#[test]
fn exhaustiveness_rejects_char_without_wildcard() {
    let src = r#"
module M
val f = { c ->
c match {
    'a' -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn exhaustiveness_rejects_partial_list() {
    let src = r#"
module M
val f = { xs ->
xs match {
    [] -> { 0 }
    [x] -> { x }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("List"), "{err}");
}

#[test]
fn exhaustiveness_accepts_list_empty_and_rest() {
    let src = r#"
module M
val f = { xs ->
xs match {
    [] -> { 0 }
    [h, ..rest] -> { h }
}
}
"#;
    let ast = parse_module(src).unwrap();
    assert!(lower_module(&ast).is_ok());
}

#[test]
fn exhaustiveness_rejects_nested_int_literal_gap() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some(3) -> { 1 }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Int"), "{err}");
}

#[test]
fn exhaustiveness_rejects_nested_partial_list() {
    let src = r#"
module M
type Option { Some(value) None }
val f = { o ->
o match {
    None -> { 0 }
    Some([a]) -> { a }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("List"), "{err}");
}

#[test]
fn with_ambiguous_product_field_defers_to_ty() {
    let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
val p = Point { x = 1, y = 2 }
p with { x = 9 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("ambiguous `with` should lower (resolve in ty)");
    let mut found_with = false;
    for it in &hir.items {
        let body = match it {
            Item::Fun(f) => &f.body,
            Item::Val { body, .. } => body,
        };
        for_each_expr(body, &mut |e| {
            if matches!(e, Expr::With { .. }) {
                found_with = true;
            }
        });
    }
    assert!(found_with, "expected deferred HIR With");
}

#[test]
fn with_unique_field_set_still_defers() {
    // Field set {x,w} uniquely matches Rect — must NOT early-expand, or
    // `Point with { x, w }` would become Rect before ty sees the base.
    let src = r#"
module M
type Point { val x val y }
type Rect { val x val w }
val main = {
val p = Point { x = 1, y = 2 }
p with { x = 7, w = 9 }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let mut found_with = false;
    let mut found_rect_new = false;
    for it in &hir.items {
        let body = match it {
            Item::Fun(f) => &f.body,
            Item::Val { body, .. } => body,
        };
        for_each_expr(body, &mut |e| {
            if matches!(e, Expr::With { .. }) {
                found_with = true;
            }
            if let Expr::AdtNew { adt_name, .. } = e {
                if adt_name == "Rect" {
                    found_rect_new = true;
                }
            }
        });
    }
    assert!(found_with, "expected deferred With");
    assert!(!found_rect_new, "must not early-expand to Rect AdtNew");
}

#[test]
fn struct_pattern_rejects_unknown_field() {
    let src = r#"
module M
type Point { val x val y }
val f = { p ->
p match {
    Point { z } -> z
    _ -> 0
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("unknown field"), "{err}");
    assert!(err.contains('z'), "{err}");
}

#[test]
fn struct_pattern_rejects_unknown_product() {
    let src = r#"
module M
type Point { val x val y }
val f = { p ->
p match {
    Piont { x } -> x
    _ -> 0
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("unknown product"), "{err}");
    assert!(err.contains("Piont"), "{err}");
}

/// Last-arm nullary ctor must still test the tag (and `MatchFail` on miss).
#[test]
fn last_arm_nullary_ctor_keeps_match_fail() {
    let src = r#"
module M
val f = { o ->
o match {
    Some(x) -> x
    None -> 0
}
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let fun = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "f" => Some(f),
            _ => None,
        })
        .expect("fun f");
    let mut has_match_fail = false;
    crate::visit::for_each_expr(&fun.body, &mut |e| {
        if matches!(
            e,
            Expr::BuiltinCall {
                name: Builtin::MatchFail,
                ..
            }
        ) {
            has_match_fail = true;
        }
    });
    assert!(
        has_match_fail,
        "last-arm `None` must remain refutable with MatchFail"
    );
}

#[test]
fn exhaustiveness_checks_instance_method_bodies() {
    let src = r#"
module M
type Option { Some(value) None }
trait Show {
val show = { self -> "" }
}
instance Show for Option {
val show = { self ->
    self match {
        None -> { "none" }
    }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
    assert!(err.contains("Some"), "{err}");
}

#[test]
fn exhaustiveness_checks_trait_default_method_bodies() {
    let src = r#"
module M
type Option { Some(value) None }
trait Show {
val show = { self ->
    self match {
        None -> { "none" }
    }
}
}
"#;
    let ast = parse_module(src).unwrap();
    let err = lower_module(&ast).unwrap_err().to_string();
    assert!(err.contains("non-exhaustive"), "{err}");
}

#[test]
fn instance_may_precede_trait_in_source_order() {
    let src = r#"
module M
type Point { val x }
instance Show for Point {
val show = { self -> "p" }
}
trait Show {
val show = { self -> "" }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("instance before trait should lower");
    assert!(hir.instances.contains(&("Show".into(), "Point".into())));
}

#[test]
fn fun_and_val_carry_declaration_span() {
    let src = "module M\nval answer = 42\nval main = { answer }\n";
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let answer = hir.items.iter().find_map(|it| match it {
        Item::Val { name, span, .. } if name == "answer" => Some(*span),
        _ => None,
    });
    let main = hir.items.iter().find_map(|it| match it {
        Item::Fun(f) if f.name == "main" => Some(f.span),
        _ => None,
    });
    let answer = answer.expect("val answer");
    let main = main.expect("fun main");
    // Decl spans should cover the `val` keyword region, not only the body literal/block.
    assert!(
        answer.start.0 < src.find("42").unwrap() as u32,
        "answer decl span {answer:?} should start at `val`"
    );
    assert!(
        main.start.0 < src.find('{').unwrap() as u32,
        "main decl span {main:?} should start at `val`"
    );
}

#[test]
fn map_filter_chain_fuses_single_builder() {
    // Keep a list sink (not `.len()`) so build fusion applies, not count fusion.
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
        }
    });
    assert_eq!(flt_acc, 0, "fused build must not emit separate filter acc");
    assert_eq!(
        map_acc, 1,
        "fused map/filter must use one list builder, got {map_acc}"
    );
}

#[test]
fn map_flatmap_chain_fuses_single_builder() {
    // Keep a list sink (not `.len()`) so flatMap build fusion applies.
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).flatMap({ x -> listOf(x, x) })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut fmap_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
        }
    });
    assert_eq!(
        map_acc, 0,
        "fused flatMap must not keep separate map builder"
    );
    assert_eq!(fmap_acc, 1, "expected one fmap builder, got {fmap_acc}");
}

#[test]
fn map_any_chain_fuses_no_map_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).any({ x -> x > 5 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused any must not keep map builder");
    assert_eq!(any_acc, 1, "expected one any acc, got {any_acc}");
}

#[test]
fn map_filter_len_fuses_count_loop() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut len_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused len must not keep map builder");
    assert_eq!(flt_acc, 0, "fused len must not keep filter builder");
    // ListLen on the *source* is expected (indexed for-each); builders must be gone.
    assert_eq!(len_acc, 1, "expected one len counter, got {len_acc}");
}

#[test]
fn map_filter_is_empty_fuses_short_circuit() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 100 }).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused isEmpty must not keep map builder");
    assert_eq!(empty_acc, 1, "expected one empty acc, got {empty_acc}");
}

#[test]
fn map_flatmap_len_fuses_count_loop() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).flatMap({ x -> listOf(x, x) }).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut len_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.len must not build concat list");
    assert_eq!(len_acc, 1, "expected one len counter, got {len_acc}");
}

#[test]
fn nested_for_in_temps_unique_under_flatmap_len_fuse() {
    // flatMap.len nests two list_for_in with the same span; index slots must not collide.
    let src = r#"
module M
val main = {
  listOf(1, 2).flatMap({ x -> listOf(x, x * 10) }).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut indices = std::collections::BTreeSet::new();
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, mutable: true, .. } = e {
            if name.starts_with(crate::desugar_slots::FOR_INDEX_PREFIX) {
                indices.insert(name.clone());
            }
        }
    });
    assert!(
        indices.len() >= 2,
        "expected distinct nested __i_ temps, got {indices:?}"
    );
}

#[test]
fn flatmap_any_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.flatMap({ x -> listOf(x, x) }).any({ y -> y > 2 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut any_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.any must not build concat list");
    assert_eq!(any_acc, 1, "expected one any acc, got {any_acc}");
}

#[test]
fn flatmap_fold_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.flatMap({ x -> listOf(x, x) }).fold(0, { a, y -> a + y })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut fuse_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__fuse_acc") {
                fuse_acc += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.fold must not build concat list");
    assert!(fuse_acc >= 1, "expected fused fold acc");
}

#[test]
fn map_filter_get_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "fused get must not keep map builder");
    assert_eq!(flt_acc, 0, "fused get must not keep filter builder");
    assert_eq!(get_acc, 1, "expected one get Option slot, got {get_acc}");
}

#[test]
fn map_flatmap_get_fuses_no_concat() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).flatMap({ x -> listOf(x, x) }).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.get must not build concat list");
    assert_eq!(get_acc, 1, "expected one get Option slot, got {get_acc}");
}

#[test]
fn let_bound_map_filter_get_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.get(0) + ys.get(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut get_acc = 0usize;
    let mut seen = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
            if name.starts_with("__fuse_seen") {
                seen += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let-bound fused get must not keep map builder");
    assert_eq!(
        flt_acc, 0,
        "let-bound fused get must not keep filter builder"
    );
    assert_eq!(get_acc, 2, "expected two get slots, got {get_acc}");
    assert_eq!(
        seen, 1,
        "two gets must share one scan, got {seen} seen counters"
    );
}

#[test]
fn lone_map_let_get_still_materializes() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  val ys = xs.map({ x -> x * 2 })
  ys.get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut par_map = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__get_acc") => get_acc += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(get_acc, 0, "lone map + let-get must not deforest");
    assert!(
        par_map >= 1,
        "lone map + let-get must keep ListParMap (Float ABI clones)"
    );
}

#[test]
fn map_filter_take_fuses_early_stop() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(flt_acc, 0, "fused take must not keep filter builder");
    assert_eq!(
        map_acc, 1,
        "fused take keeps one list builder, got {map_acc}"
    );
    assert_eq!(take_k, 1, "expected take counter, got {take_k}");
}

#[test]
fn map_flatmap_take_fuses_no_concat() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).flatMap({ x -> listOf(x, x) }).take(2)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.take must not build concat list");
    assert_eq!(take_k, 1, "expected take counter, got {take_k}");
}

#[test]
fn let_bound_get_and_len_share_scan() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.get(0) + ys.len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut len_acc = 0usize;
    let mut get_acc = 0usize;
    let mut seen = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
            if name.starts_with("__fuse_seen") {
                seen += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "get+len must not keep map builder");
    assert_eq!(len_acc, 0, "get+len must not keep a separate len loop");
    assert_eq!(get_acc, 1, "expected one get slot, got {get_acc}");
    assert_eq!(
        seen, 1,
        "get+len must share one scan, got {seen} seen counters"
    );
}

#[test]
fn iota_let_map_get_deforests() {
    let src = r#"
module M
val main = {
  val ys = (1..10).map({ x -> x * 2 })
  ys.get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut par_map = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__get_acc") => get_acc += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(par_map, 0, "iota map+get must not par_map materialize");
    assert_eq!(get_acc, 1, "expected fused get on iota map");
}

#[test]
fn iota_let_map_len_and_gets_materializes() {
    let src = r#"
module M
val main = {
  val doubled = (1..5).map({ x -> x * 2 })
  doubled.len()
  doubled.get(0)
  doubled.get(4)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut par_map = 0usize;
    let mut fuse_seen = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__fuse_seen") => fuse_seen += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(fuse_seen, 0, "len+gets on iota map must not shared-scan");
    assert!(
        par_map >= 1,
        "len+gets on iota lone map must keep ListParMap (range_map golden)"
    );
}

#[test]
fn iota_let_filter_len_and_gets_builds() {
    let src = r#"
module M
val main = {
  val odds = (0..<10).filter({ x -> x % 2 == 1 })
  odds.len()
  odds.get(0)
  odds.get(4)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut flt_acc = 0usize;
    let mut fuse_seen = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__flt_acc") => flt_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__fuse_seen") => fuse_seen += 1,
        _ => {}
    });
    assert_eq!(fuse_seen, 0, "len+gets on iota filter must not shared-scan");
    assert_eq!(flt_acc, 1, "expected filter builder for range_map odds");
}

#[test]
fn map_filter_drop_fuses_skip_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).drop(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(flt_acc, 0, "fused drop must not keep filter builder");
    assert_eq!(
        map_acc, 1,
        "fused drop keeps one list builder, got {map_acc}"
    );
    assert_eq!(drop_k, 1, "expected drop skip counter, got {drop_k}");
}

#[test]
fn map_filter_slice_fuses_like_drop() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x }).filter({ x -> true }).slice(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut drop_k = 0usize;
    let mut saw_slice = false;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__drop_k") => drop_k += 1,
        Expr::BuiltinCall {
            name: Builtin::ListSlice,
            ..
        } => saw_slice = true,
        _ => {}
    });
    assert_eq!(drop_k, 1, "slice should fuse like drop");
    assert!(!saw_slice, "fused slice must not emit ListSlice builtin");
}

#[test]
fn map_flatmap_drop_fuses_no_concat() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).flatMap({ x -> listOf(x, x) }).drop(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "fused flatMap.drop must not build concat list");
    assert_eq!(drop_k, 1, "expected drop skip counter, got {drop_k}");
}

#[test]
fn map_filter_take_get_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(2).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut get_acc = 0usize;
    let mut take_n = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
            if name.starts_with("__take_n") {
                take_n += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "take.get must not materialize map builder");
    assert_eq!(get_acc, 1, "expected fused get slot");
    assert!(take_n >= 1, "expected take lim bind");
}

#[test]
fn map_filter_drop_get_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).drop(1).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut get_acc = 0usize;
    let mut drop_n = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
            if name.starts_with("__drop_n") {
                drop_n += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.get must not materialize map builder");
    assert_eq!(get_acc, 1, "expected fused get slot");
    assert!(drop_n >= 1, "expected drop lim bind");
}

#[test]
fn map_filter_drop_take_get_fuses() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4, 5)
  xs.map({ x -> x }).filter({ x -> true }).drop(1).take(2).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.take.get must not materialize");
    assert_eq!(get_acc, 1, "expected fused get");
}

#[test]
fn map_filter_take_len_fuses_capped() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(1).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut len_acc = 0usize;
    let mut take_n = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__take_n") {
                take_n += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "take.len must not keep map builder");
    assert_eq!(len_acc, 1, "expected capped len counter");
    assert!(take_n >= 1, "expected take lim");
}

#[test]
fn map_filter_drop_len_fuses_skip() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).drop(1).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut len_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.len must not keep map builder");
    assert_eq!(len_acc, 1, "expected skip-then-count len");
    assert_eq!(drop_k, 1, "expected drop skip counter");
}

#[test]
fn map_filter_drop_take_fuses_skip_then_fill() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4, 5)
  xs.map({ x -> x }).filter({ x -> true }).drop(1).take(2)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut drop_k = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 1, "drop.take keeps one builder");
    assert_eq!(drop_k, 1, "expected drop skip");
    assert_eq!(take_k, 1, "expected take counter");
}

#[test]
fn let_bound_take_and_drop_deforest() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.take(1).len() + ys.drop(1).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut take_k = 0usize;
    let mut drop_k = 0usize;
    let mut len_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let take.len + drop.len must not materialize");
    assert_eq!(take_k, 0, "take.len is a capped count, not a take builder");
    assert_eq!(drop_k, 1, "expected fused drop skip");
    assert_eq!(len_acc, 2, "expected two count loops");
}

#[test]
fn map_filter_take_is_empty_short_circuits() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  xs.map({ x -> x }).filter({ x -> x > 0 }).take(0).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut take_n = 0usize;
    let mut empty_acc = 0usize;
    let mut saw_true = false;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__take_n") => take_n += 1,
        Expr::Let { name, .. } if name.starts_with("__empty_acc") => empty_acc += 1,
        Expr::Bool(true, _) => saw_true = true,
        _ => {}
    });
    assert_eq!(take_n, 0, "literal take(0).isEmpty must constant-fold");
    assert_eq!(empty_acc, 0, "take(0).isEmpty must not scan");
    assert!(saw_true, "take(0).isEmpty lowers to true");
}

#[test]
fn map_filter_drop_is_empty_short_circuits() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).drop(1).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut len_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.isEmpty must not keep map builder");
    assert_eq!(empty_acc, 1, "drop(n).isEmpty short-circuits after skip");
    assert_eq!(len_acc, 0, "must not count remaining after drop");
    assert_eq!(drop_k, 1, "expected fused drop skip");
}

#[test]
fn map_filter_drop_drop_is_empty_short_circuits() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).drop(1).drop(1).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut len_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.drop.isEmpty must not keep map builder");
    assert_eq!(
        empty_acc, 1,
        "nested drop.isEmpty short-circuits after summed skip"
    );
    assert_eq!(len_acc, 0, "must not count remaining after nested drop");
    assert_eq!(drop_k, 1, "nested drop must share one skip");
}

#[test]
fn map_filter_take_drop_is_empty_short_circuits() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(5).drop(1).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut len_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "take.drop.isEmpty must not keep map builder");
    assert_eq!(empty_acc, 1, "take.drop.isEmpty short-circuits after skip");
    assert_eq!(len_acc, 0, "must not count remaining after take.drop");
    assert_eq!(drop_k, 1, "expected fused drop skip");
}

#[test]
fn map_filter_take_drop_exhausted_is_true() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(1).drop(1).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut empty_acc = 0usize;
    let mut len_acc = 0usize;
    let mut saw_true = false;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__empty_acc") => empty_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__len_acc") => len_acc += 1,
        Expr::Bool(true, _) => saw_true = true,
        _ => {}
    });
    assert_eq!(empty_acc, 0, "take(1).drop(1).isEmpty must not scan");
    assert_eq!(len_acc, 0, "must not count");
    assert!(saw_true, "take(n).drop(n).isEmpty lowers to true");
}

#[test]
fn iota_map_filter_len_fuses() {
    let src = r#"
module M
val main = {
  (1..10).map({ x -> x * 2 }).filter({ x -> x > 5 }).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut len_acc = 0usize;
    let mut par_map = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__len_acc") => len_acc += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(map_acc, 0, "iota map.filter.len must not build");
    assert_eq!(len_acc, 1, "expected count loop");
    assert_eq!(par_map, 0, "must not par_map");
}

#[test]
fn map_filter_take_any_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(2).any({ x -> x > 10 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "take.any must not keep map builder");
    assert_eq!(any_acc, 1, "expected fused any");
    assert_eq!(take_k, 1, "expected take cap counter");
}

#[test]
fn map_filter_drop_any_fuses_skip() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).drop(1).any({ x -> x > 0 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.any must not keep map builder");
    assert_eq!(any_acc, 1, "expected fused any");
    assert_eq!(drop_k, 1, "expected drop skip counter");
}

#[test]
fn map_filter_take_fold_fuses_early_stop() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x }).filter({ x -> true }).take(2).fold(0, { a, x -> a + x })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut fuse_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__fuse_acc") {
                fuse_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "take.fold must not keep map builder");
    assert_eq!(fuse_acc, 1, "expected fused fold");
    assert_eq!(take_k, 1, "expected take cap counter");
}

#[test]
fn map_filter_drop_fold_fuses_skip() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x }).filter({ x -> true }).drop(1).fold(0, { a, x -> a + x })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut fuse_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__fuse_acc") {
                fuse_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.fold must not keep map builder");
    assert_eq!(fuse_acc, 1, "expected fused fold");
    assert_eq!(drop_k, 1, "expected drop skip counter");
}

#[test]
fn let_bound_take_get_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.take(2).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut get_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let take.get must not materialize pipe");
    assert_eq!(get_acc, 1, "expected fused get under take");
    assert_eq!(take_k, 0, "take.get must not fill a take builder");
}

#[test]
fn let_bound_drop_len_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.drop(1).len()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut len_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let drop.len must not materialize pipe");
    assert_eq!(len_acc, 1, "expected skip-then-count");
    assert_eq!(drop_k, 1, "expected drop skip counter");
}

#[test]
fn iota_map_take_fuses() {
    let src = r#"
module M
val main = {
  (1..10).map({ x -> x * 2 }).take(3)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut take_k = 0usize;
    let mut par_map = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__take_k") => take_k += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(par_map, 0, "iota map.take must not par_map");
    assert_eq!(map_acc, 1, "fused take keeps one builder");
    assert_eq!(take_k, 1, "expected take counter");
}

#[test]
fn iota_map_filter_get_fuses() {
    let src = r#"
module M
val main = {
  (1..20).map({ x -> x * 2 }).filter({ x -> x > 5 }).get(0)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut get_acc = 0usize;
    let mut par_map = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__get_acc") => get_acc += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(par_map, 0, "iota map.filter.get must not par_map");
    assert_eq!(map_acc, 0, "must not materialize");
    assert_eq!(get_acc, 1, "expected fused get");
}

#[test]
fn map_filter_drop_drop_fuses_sum() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4, 5)
  xs.map({ x -> x }).filter({ x -> true }).drop(1).drop(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut drop_a = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__drop_a") {
                drop_a += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(drop_a, 1, "drop.drop binds summed skip");
    assert_eq!(drop_k, 1, "single skip loop");
}

#[test]
fn map_filter_contains_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).contains(6)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    let mut contains = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__any_acc") => any_acc += 1,
        Expr::BuiltinCall {
            name: Builtin::Contains,
            ..
        } => contains += 1,
        _ => {}
    });
    assert_eq!(map_acc, 0, "contains must not keep map builder");
    assert_eq!(any_acc, 1, "contains lowers to fused any");
    assert_eq!(contains, 0, "must not emit Contains builtin");
}

#[test]
fn map_filter_take_contains_fuses() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(2).contains(6)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "take.contains must not materialize");
    assert_eq!(any_acc, 1, "expected fused any");
    assert_eq!(take_k, 1, "expected take cap");
}

#[test]
fn let_bound_contains_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.contains(8)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let contains must not materialize");
    assert_eq!(any_acc, 1, "expected fused any");
}

#[test]
fn let_bound_any_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.any({ x -> x > 5 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut any_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__any_acc") {
                any_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let any must not materialize pipe");
    assert_eq!(any_acc, 1, "expected fused any");
}

#[test]
fn let_bound_all_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.all({ x -> x > 0 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut all_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__all_acc") {
                all_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let all must not materialize pipe");
    assert_eq!(all_acc, 1, "expected fused all");
}

#[test]
fn let_bound_find_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.find({ x -> x > 5 })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut find_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__find_acc") {
                find_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let find must not materialize pipe");
    assert_eq!(find_acc, 1, "expected fused find");
}

#[test]
fn let_bound_fold_assoc_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.fold(0, { a, x -> a + x })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut fuse_x = 0usize;
    let mut par_fold = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__fuse_x_") => fuse_x += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParFold,
            ..
        } => par_fold += 1,
        _ => {}
    });
    assert_eq!(map_acc, 0, "let fold must not materialize pipe");
    assert_eq!(par_fold, 0, "let-bound pipe fold must not keep ListParFold");
    assert!(fuse_x >= 1, "expected fused scan");
}

#[test]
fn let_bound_take_fold_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.take(2).fold(0, { a, x -> a + x })
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut par_fold = 0usize;
    let mut fuse_x = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__fuse_x_") => fuse_x += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParFold,
            ..
        } => par_fold += 1,
        _ => {}
    });
    assert_eq!(map_acc, 0, "let take.fold must not materialize pipe");
    assert_eq!(par_fold, 0, "must not keep ListParFold on take");
    assert!(fuse_x >= 1, "expected fused scan");
}

#[test]
fn let_bound_drop_drop_one_skip() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4, 5)
  val ys = xs.map({ x -> x }).filter({ x -> true })
  ys.drop(1).drop(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 1, "drop.drop keeps one skip builder");
    assert_eq!(drop_k, 1, "nested drop must share one skip loop");
}

#[test]
fn let_bound_take_take_one_fill() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4, 5)
  val ys = xs.map({ x -> x }).filter({ x -> true })
  ys.take(3).take(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 1, "take.take keeps one fill builder");
    assert_eq!(take_k, 1, "nested take must share one fill loop");
}

#[test]
fn let_bound_drop_drop_is_empty_short_circuits() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.drop(1).drop(1).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "drop.drop.isEmpty must not build a list");
    assert_eq!(empty_acc, 1, "expected fused isEmpty after summed skip");
    assert_eq!(drop_k, 1, "nested drop must share one skip");
}

#[test]
fn let_bound_take_drop_is_empty_short_circuits() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.take(5).drop(1).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut len_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "take.drop.isEmpty must not build a list");
    assert_eq!(empty_acc, 1, "expected fused isEmpty after skip");
    assert_eq!(len_acc, 0, "must not count remaining after take.drop");
    assert_eq!(drop_k, 1, "expected fused drop skip");
}

#[test]
fn let_bound_take_drop_one_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4, 5)
  val ys = xs.map({ x -> x }).filter({ x -> true })
  ys.take(3).drop(1)
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut drop_k = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 1, "take.drop keeps one skip+fill builder");
    assert_eq!(drop_k, 1, "expected fused drop skip");
    assert_eq!(take_k, 1, "expected fused take counter");
}

#[test]
fn let_bound_is_empty_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 100 })
  ys.isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let isEmpty must not materialize pipe");
    assert_eq!(empty_acc, 1, "expected fused isEmpty short-circuit scan");
}

#[test]
fn let_bound_take_is_empty_short_circuits() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.take(2).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut len_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let take.isEmpty must not materialize pipe");
    assert_eq!(empty_acc, 1, "take(k>0).isEmpty ≡ inner short-circuit scan");
    assert_eq!(len_acc, 0, "must not count the take prefix");
}

#[test]
fn let_bound_take0_is_empty_is_true() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 0 })
  ys.take(0).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut saw_true = false;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__empty_acc") => empty_acc += 1,
        Expr::Bool(true, _) => saw_true = true,
        _ => {}
    });
    assert_eq!(map_acc, 0, "take(0).isEmpty must not materialize");
    assert_eq!(empty_acc, 0, "literal take(0).isEmpty must not scan");
    assert!(saw_true, "take(0).isEmpty lowers to true");
}

#[test]
fn let_bound_drop_is_empty_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  ys.drop(1).isEmpty()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut empty_acc = 0usize;
    let mut len_acc = 0usize;
    let mut drop_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__empty_acc") {
                empty_acc += 1;
            }
            if name.starts_with("__len_acc") {
                len_acc += 1;
            }
            if name.starts_with("__drop_k") {
                drop_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let drop.isEmpty must not materialize pipe");
    assert_eq!(empty_acc, 1, "drop(n).isEmpty short-circuits after skip");
    assert_eq!(len_acc, 0, "must not count remaining after drop");
    assert_eq!(drop_k, 1, "expected fused drop skip");
}

#[test]
fn let_bound_get_in_loop_still_materializes() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  var i = 0
  for i < 2 {
    ys.get(i)
    i = i + 1
  }
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut get_acc = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__get_acc") {
                get_acc += 1;
            }
        }
    });
    assert!(
        map_acc >= 1,
        "loop-hosted get must keep materialized pipe, got {map_acc}"
    );
    assert_eq!(get_acc, 0, "must not pre-scan loop gets");
}

#[test]
fn for_in_map_filter_fuses_no_builder() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  var s = 0
  for y in xs.map({ x -> x * 2 }).filter({ x -> x > 2 }) {
    s = s + y
  }
  s
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut flt_acc = 0usize;
    let mut fuse_x = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__flt_acc") {
                flt_acc += 1;
            }
            if name.starts_with("__fuse_x_") {
                fuse_x += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "for-in must not keep map builder");
    assert_eq!(flt_acc, 0, "for-in must not keep filter builder");
    assert!(fuse_x >= 1, "expected fused for-in scan");
}

#[test]
fn for_in_lone_map_skips_par_map() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  var s = 0
  for y in xs.map({ x -> x * 2 }) {
    s = s + y
  }
  s
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut par_map = 0usize;
    let mut map_acc = 0usize;
    let mut fuse_x = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::Let { name, .. } if name.starts_with("__fuse_x_") => fuse_x += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(par_map, 0, "sequential for-in must not par_map");
    assert_eq!(map_acc, 0, "for-in lone map must not build a list");
    assert!(fuse_x >= 1, "expected fused for-in scan");
}

#[test]
fn for_in_map_filter_take_fuses() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4, 5)
  var s = 0
  for y in xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).take(2) {
    s = s + y
  }
  s
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut take_k = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__take_k") {
                take_k += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "for-in take must not materialize");
    assert_eq!(take_k, 1, "expected take cap");
}

#[test]
fn for_in_map_flatmap_fuses_no_concat() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3)
  var s = 0
  for y in xs.map({ x -> x }).flatMap({ x -> listOf(x, x) }) {
    s = s + y
  }
  s
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut fmap_acc = 0usize;
    let mut for_done = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__fmap_acc") {
                fmap_acc += 1;
            }
            if name.starts_with("__for_done") {
                for_done += 1;
            }
        }
    });
    assert_eq!(fmap_acc, 0, "for-in flatMap must not concat");
    assert_eq!(for_done, 1, "expected done flag for break");
}

#[test]
fn let_bound_for_in_deforests() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val ys = xs.map({ x -> x * 2 }).filter({ x -> x > 2 })
  var s = 0
  for y in ys {
    s = s + y
  }
  s
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut fuse_x = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__fuse_x_") {
                fuse_x += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "let-bound for-in must not materialize");
    assert!(fuse_x >= 1, "expected fused scan");
}

#[test]
fn map_filter_toset_fuses_no_list() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).toSet()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut toset = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__toset_acc") {
                toset += 1;
            }
        }
    });
    assert_eq!(map_acc, 0, "toSet must not keep list builder");
    assert_eq!(toset, 1, "expected set accumulator");
}

#[test]
fn map_filter_tolist_skips_copy_pass() {
    let src = r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).toList()
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut map_acc = 0usize;
    let mut tolist = 0usize;
    for_each_expr(body, &mut |e| {
        if let Expr::Let { name, .. } = e {
            if name.starts_with("__map_acc") {
                map_acc += 1;
            }
            if name.starts_with("__tolist_acc") {
                tolist += 1;
            }
        }
    });
    assert_eq!(map_acc, 1, "toList is one fused builder");
    assert_eq!(tolist, 0, "must not copy through toList acc");
}

#[test]
fn iota_map_for_in_fuses() {
    let src = r#"
module M
val main = {
  var s = 0
  for y in (1..5).map({ x -> x * 2 }) {
    s = s + y
  }
  s
}
"#;
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    let body = hir
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(&f.body),
            _ => None,
        })
        .expect("main");
    let mut par_map = 0usize;
    let mut map_acc = 0usize;
    for_each_expr(body, &mut |e| match e {
        Expr::Let { name, .. } if name.starts_with("__map_acc") => map_acc += 1,
        Expr::BuiltinCall {
            name: Builtin::ListParMap,
            ..
        } => par_map += 1,
        _ => {}
    });
    assert_eq!(par_map, 0, "iota for-in must not par_map");
    assert_eq!(map_acc, 0, "iota for-in must not build");
}

fn hir_main_body(src: &str) -> Expr {
    let ast = parse_module(src).unwrap();
    let hir = lower_module(&ast).expect("lower");
    hir.items
        .iter()
        .find_map(|it| match it {
            Item::Fun(f) if f.name == "main" => Some(f.body.clone()),
            _ => None,
        })
        .expect("main")
}

fn count_lets_with_prefix(e: &Expr, prefix: &str) -> usize {
    let mut n = 0usize;
    for_each_expr(e, &mut |x| {
        if let Expr::Let { name, .. } = x {
            if name.starts_with(prefix) {
                n += 1;
            }
        }
    });
    n
}

#[test]
fn map_filter_tomap_get_fuses_no_hash() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf((1, 10), (2, 20), (3, 30))
  xs.filter({ p -> p.1 > 0 }).toMap().get(2)
}
"#,
    );
    assert_eq!(count_lets_with_prefix(&body, "__map_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__tomap_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__mget_acc"), 1);
}

#[test]
fn pairs_tomap_get_scans_without_stages() {
    let body = hir_main_body(
        r#"
module M
val main = {
  listOf((1, 10), (2, 20)).toMap().get(2)
}
"#,
    );
    assert_eq!(count_lets_with_prefix(&body, "__tomap_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__mget_acc"), 1);
}

#[test]
fn map_filter_tomap_contains_fuses() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf((1, 10), (2, 20))
  xs.filter({ p -> p.1 > 0 }).toMap().contains(2)
}
"#,
    );
    assert_eq!(count_lets_with_prefix(&body, "__tomap_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__mcontains_acc"), 1);
}

#[test]
fn let_bound_tomap_get_deforests() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf((1, 10), (2, 20), (2, 30))
  val m = xs.filter({ p -> p.1 > 0 }).toMap()
  m.get(2)
}
"#,
    );
    assert_eq!(count_lets_with_prefix(&body, "__tomap_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__mget_acc"), 1);
}

#[test]
fn let_bound_tomap_two_gets_keeps_hash() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf((1, 10), (2, 20))
  val m = xs.filter({ p -> p.1 > 0 }).toMap()
  m.get(1)
  m.get(2)
}
"#,
    );
    assert!(
        count_lets_with_prefix(&body, "__tomap_acc") >= 1,
        "two Map gets must keep Hash"
    );
    assert_eq!(count_lets_with_prefix(&body, "__mget_acc"), 0);
}

#[test]
fn take_tomap_get_fuses() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf((1, 10), (2, 20), (3, 30))
  xs.filter({ p -> p.1 > 0 }).take(2).toMap().get(2)
}
"#,
    );
    assert_eq!(count_lets_with_prefix(&body, "__tomap_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__mget_acc"), 1);
}

#[test]
fn map_filter_toset_contains_fuses() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).toSet().contains(8)
}
"#,
    );
    assert_eq!(count_lets_with_prefix(&body, "__map_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__toset_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__any_acc"), 1);
}

#[test]
fn let_bound_toset_contains_deforests() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val s = xs.map({ x -> x * 2 }).filter({ x -> x > 2 }).toSet()
  s.contains(8)
}
"#,
    );
    assert_eq!(count_lets_with_prefix(&body, "__toset_acc"), 0);
    assert_eq!(count_lets_with_prefix(&body, "__any_acc"), 1);
}

#[test]
fn let_bound_toset_two_contains_keeps_set() {
    let body = hir_main_body(
        r#"
module M
val main = {
  val xs = listOf(1, 2, 3, 4)
  val s = xs.filter({ x -> x > 1 }).toSet()
  s.contains(2)
  s.contains(3)
}
"#,
    );
    assert!(
        count_lets_with_prefix(&body, "__toset_acc") >= 1,
        "two Set contains must keep the set"
    );
}

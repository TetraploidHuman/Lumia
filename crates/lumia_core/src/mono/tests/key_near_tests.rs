use super::MonoKind;

#[test]
fn mono_kind_encode_stable_keys() {
    assert_eq!(MonoKind::Int.encode(), "Int");
    assert_eq!(
        MonoKind::List(Box::new(MonoKind::Float)).encode(),
        "List_Float"
    );
    assert_eq!(
        MonoKind::Map(Box::new(MonoKind::String), Box::new(MonoKind::Int)).encode(),
        "Map_String_Int"
    );
    assert_eq!(
        MonoKind::Adt {
            name: "Option".into(),
            params: vec![MonoKind::Float],
        }
        .encode(),
        "Option_Float"
    );
    assert_eq!(MonoKind::FunRef("a.b$c".into()).encode(), "Fn_a_b_c");
    assert_eq!(MonoKind::Unit.encode(), "Unit");
}


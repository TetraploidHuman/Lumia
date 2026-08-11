use super::*;

#[test]
fn const_fold_adt_tag() {
    use lumia_core::AdtRepr;
    let mut module = CoreModule::with_functions(
        "C",
        vec![bare_fun(
            "f",
            vec![],
            Block {
                params: vec![],
                ops: vec![
                    Op::Let {
                        local: Local(0),
                        value: Value::AllocAdt {
                            adt_name: "Color".into(),
                            tag: 2,
                            fields: vec![],
                            repr: AdtRepr::LitAdt,
                        },
                        pure_region: true,
                    },
                    Op::Let {
                        local: Local(1),
                        value: Value::Builtin {
                            name: Builtin::AdtTag,
                            args: vec![Local(0)],
                        },
                        pure_region: true,
                    },
                ],
                result: Some(Local(1)),
            },
        )],
    );
    ConstFoldPass.run(&mut module);
    assert!(matches!(
        &module.functions[0].body.ops[1],
        Op::Let {
            value: Value::Int(2),
            ..
        }
    ));
}

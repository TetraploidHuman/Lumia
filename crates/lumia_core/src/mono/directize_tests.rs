    use super::*;
    use crate::ir::{CoreFun, FunKind};
    use lumia_ty::{Effect, Type};
    use rustc_hash::FxHashSet as HashSet;

    fn fun(body: Block) -> CoreFun {
        CoreFun {
            name: "f".into(),
            params: vec![],
            param_names: vec![],
            param_tys: vec![],
            body,
            ret_ty: Type::Int,
            effect: Effect::pure(),
            is_main: false,
            memo: None,
            external: None,
            foreign_abi: crate::ForeignAbi::C,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        }
    }

    #[test]
    fn directizes_local_funref_to_call() {
        let body = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::FunRef("g".into()),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::IndirectCall {
                        callee: Local(0),
                        args: vec![Local(2)],
                    },
                    pure_region: true,
                },
            ],
            result: Some(Local(1)),
        };
        let mut module = CoreModule::empty("m");
        module.functions.push(fun(body));
        directize_funref_calls(&mut module);
        let f = &module.functions[0];
        assert!(
            matches!(
                &f.body.ops[1],
                Op::Let {
                    value: Value::Call { fun, args },
                    ..
                } if fun == "g" && args == &vec![Local(2)]
            ),
            "expected Call(g), got {:?}",
            f.body.ops[1]
        );
    }

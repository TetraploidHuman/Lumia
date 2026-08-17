    use super::*;
    use lumia_core::{Block, CoreFun, CoreModule, FunKind, Local, Op, Value};
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
            foreign_abi: lumia_core::ForeignAbi::C,
            escaping: HashSet::default(),
            scheme_poly: false,
            mono_of: None,
            kind: FunKind::Normal,
        }
    }

    #[test]
    fn collapses_local_copy_chain() {
        let body = Block {
            ops: vec![
                Op::Let {
                    local: Local(0),
                    value: Value::Int(7),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(1),
                    value: Value::Local(Local(0)),
                    pure_region: true,
                },
                Op::Let {
                    local: Local(2),
                    value: Value::Local(Local(1)),
                    pure_region: true,
                },
            ],
            result: Some(Local(2)),
        };
        let mut module = CoreModule::empty("m");
        module.functions.push(fun(body));
        CopyElimPass.run(&mut module);
        let f = &module.functions[0];
        assert_eq!(f.body.ops.len(), 1, "copy lets stripped: {:?}", f.body.ops);
        assert!(matches!(
            &f.body.ops[0],
            Op::Let {
                local: Local(0),
                value: Value::Int(7),
                ..
            }
        ));
        assert_eq!(f.body.result, Some(Local(0)));
    }

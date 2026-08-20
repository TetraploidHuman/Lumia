use crate::compile_source_to_core;
use crate::lambda_lift::float_abi::collect_fun_cap_tys;
use crate::ModuleTables;
use lumia_ty::Type;

/// Nested `{ x -> x + k }` inside `make(k)` is lifted before mono sees Float.
/// Post-mono ABI must come from typed cap tables — IR has no `as_float` flag.
#[test]
fn nested_float_param_capture_typed_cap_after_pipeline() {
    let core = compile_source_to_core(
        r#"
module M
val make = { k ->
  { x -> x + k }
}
val main = {
  make(1.5)(2.0)
}
"#,
    )
    .expect("core");
    let tables = ModuleTables::from_module(&core);
    let cap_tys = collect_fun_cap_tys(&core, &tables.fun_ret_tys, &tables.fun_param_tys);
    let float_caps: Vec<_> = cap_tys
        .iter()
        .flat_map(|(fun, m)| {
            m.iter()
                .filter(|(_, t)| matches!(t, Type::Float))
                .map(move |(i, _)| (fun.clone(), *i))
        })
        .collect();
    assert!(
        !float_caps.is_empty(),
        "expected typed Float capture after pipeline; cap_tys={cap_tys:?} funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
}

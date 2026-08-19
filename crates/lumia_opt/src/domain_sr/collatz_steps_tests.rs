use super::CollatzStepsPass;
use crate::{compile_source_to_optimized, OptOptions};

const COLLATZ_STEPS_SRC: &str = r#"
module M
val collatzSteps(n) = {
  var x = n
  var steps = 0
  for x > 1 {
    if x % 2 == 0 {
      x = x / 2
    } else {
      x = 3 * x + 1
    }
    steps = steps + 1
  }
  steps
}
val main = collatzSteps(27)
"#;

fn has_collatz_steps_rt_call(core: &lumia_core::CoreModule) -> bool {
    core.functions.iter().any(|f| {
        f.body.ops.iter().any(|op| {
            matches!(
                op,
                lumia_core::Op::Let {
                    value: lumia_core::Value::Call { fun, .. },
                    ..
                } if fun == "lumia_collatz_steps"
            )
        })
    })
}

#[test]
fn rewrites_collatz_steps_on_unoptimized_core() {
    let mut core = lumia_core::compile_source_to_core(COLLATZ_STEPS_SRC).expect("core");
    CollatzStepsPass.run(&mut core);
    assert!(has_collatz_steps_rt_call(&core), "expected RT rewrite");
}

#[test]
fn debug_pipeline_rewrites_collatz_steps() {
    let core =
        compile_source_to_optimized(COLLATZ_STEPS_SRC, &OptOptions::for_build(false)).unwrap();
    assert!(
        has_collatz_steps_rt_call(&core),
        "Debug pipeline must rewrite collatzSteps after codegen cttz removal"
    );
}

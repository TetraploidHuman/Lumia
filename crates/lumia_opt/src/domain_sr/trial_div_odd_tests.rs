use super::super::match_primes::match_trial_div_loop;
use super::TrialDivOddPass;
use crate::{compile_source_to_optimized, OptOptions};
use lumia_core::{
    collect_leaf_defs, collect_loops, const_of, for_each_block_dfs, is_unit_inc, name_of, Block,
    CoreBinOp, CoreModule, Local, Op, Value,
};
use rustc_hash::FxHashMap as HashMap;

const ISPRIME_SRC: &str = r#"
module M
val isPrime(n) = {
    if n < 2 {
        false
    } else {
        var d = 2
        var ok = true
        for d * d <= n {
            if n % d == 0 {
                ok = false
                break
            }
            d = d + 1
        }
        ok
    }
}
val main = {
    var n = 97
    isPrime(n)
}
"#;

fn is_prime_fun(core: &CoreModule) -> &lumia_core::CoreFun {
    core.functions
        .iter()
        .find(|f| f.name == "isPrime" || f.name.starts_with("isPrime$"))
        .unwrap_or_else(|| {
            panic!(
                "missing isPrime, funs={:?}",
                core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
            )
        })
}

fn add_name_plus_two(dest: u32, defs: &HashMap<u32, Value>) -> Option<String> {
    let Value::Binary {
        op: CoreBinOp::Add,
        left,
        right,
    } = defs.get(&dest)?
    else {
        return None;
    };
    if const_of(*right, defs) == Some(2) {
        name_of(*left, defs)
    } else if const_of(*left, defs) == Some(2) {
        name_of(*right, defs)
    } else {
        None
    }
}

fn has_odd_step(block: &Block, defs: &HashMap<u32, Value>) -> bool {
    let mut found = false;
    for_each_block_dfs(block, &mut |b| {
        for op in &b.ops {
            let Op::Let {
                value:
                    Value::If {
                        cond,
                        then_block,
                        else_block,
                    },
                ..
            } = op
            else {
                continue;
            };
            let Some(then_r) = then_block.result else {
                continue;
            };
            let Some(else_r) = else_block.result else {
                continue;
            };
            if const_of(then_r, defs) != Some(3) {
                continue;
            }
            let Some(slot) = add_name_plus_two(else_r.0, defs) else {
                continue;
            };
            let Value::Binary {
                op: CoreBinOp::Eq,
                left,
                right,
            } = defs.get(&cond.0).cloned().unwrap_or(Value::Unit)
            else {
                continue;
            };
            let eq_name = name_of(left, defs).or_else(|| name_of(right, defs));
            let eq_two = const_of(left, defs) == Some(2) || const_of(right, defs) == Some(2);
            if eq_name.as_deref() == Some(slot.as_str()) && eq_two {
                found = true;
            }
        }
    });
    found
}

fn remaining_unit_trial(core: &CoreModule) -> usize {
    let mut n = 0;
    for f in &core.functions {
        if f.external.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&f.body, false);
        let mut loops = vec![];
        collect_loops(&f.body, &mut loops);
        for (h, b, l) in &loops {
            if match_trial_div_loop(h, b, l, &defs).is_some() {
                n += 1;
            }
        }
    }
    n
}

fn leftover_d_unit_inc(f: &lumia_core::CoreFun) -> bool {
    let defs = collect_leaf_defs(&f.body, false);
    let mut hit = false;
    for_each_block_dfs(&f.body, &mut |b| {
        for op in &b.ops {
            if let Op::Assign {
                name,
                value: Local(v),
            } = op
            {
                if name == "d" && is_unit_inc(*v, "d", &defs) {
                    hit = true;
                }
            }
        }
    });
    hit
}

#[test]
fn rewrites_is_prime_loop_on_unoptimized_core() {
    let mut core = lumia_core::compile_source_to_core(ISPRIME_SRC).expect("core");
    assert!(
        remaining_unit_trial(&core) >= 1,
        "fixture should match trial-div before rewrite"
    );
    TrialDivOddPass.run(&mut core);
    let f = is_prime_fun(&core);
    let defs = collect_leaf_defs(&f.body, false);
    assert!(
        has_odd_step(&f.body, &defs),
        "expected (d==2)?3:d+2, ops={:?}",
        f.body.ops
    );
    assert!(!leftover_d_unit_inc(f), "unit d+=1 should be gone");
    assert_eq!(remaining_unit_trial(&core), 0);
}

#[test]
fn debug_pipeline_rewrites_live_is_prime() {
    let core = compile_source_to_optimized(ISPRIME_SRC, &OptOptions::for_build(false)).unwrap();
    let f = is_prime_fun(&core);
    let defs = collect_leaf_defs(&f.body, false);
    assert!(
        has_odd_step(&f.body, &defs),
        "Debug pipeline should odd-step isPrime"
    );
    assert!(!leftover_d_unit_inc(f));
}

#[test]
fn release_pipeline_rewrites_live_is_prime() {
    let core = compile_source_to_optimized(ISPRIME_SRC, &OptOptions::for_build(true)).unwrap();
    // Inline may absorb isPrime into main; search any remaining body.
    let mut found = false;
    for f in &core.functions {
        if f.external.is_some() {
            continue;
        }
        let defs = collect_leaf_defs(&f.body, false);
        if has_odd_step(&f.body, &defs) {
            found = true;
            assert!(!leftover_d_unit_inc(f), "{}", f.name);
        }
    }
    assert!(
        found,
        "Release should keep a live odd-step loop (isPrime or inlined), funs={:?}",
        core.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    assert_eq!(remaining_unit_trial(&core), 0);
}

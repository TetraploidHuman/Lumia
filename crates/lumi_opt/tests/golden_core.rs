//! Core IR golden snapshots for high-signal examples.
//!
//! Regenerate with: `UPDATE_GOLDEN=1 cargo test -p lumi_opt --test golden_core`

use lumi_core::format_module;
use lumi_opt::{compile_file_to_optimized, OptOptions};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    lumi_abi::workspace_root_canonical(env!("CARGO_MANIFEST_DIR"))
}

fn golden_path(stem: &str) -> PathBuf {
    workspace_root().join(format!("tests/golden/core/{stem}.ir"))
}

fn assert_core_golden(rel_example: &str, release: bool) {
    let root = workspace_root();
    let src_path = root.join(rel_example);
    let stem = Path::new(rel_example)
        .file_stem()
        .and_then(|s| s.to_str())
        .expect("stem");
    let opts = if release {
        OptOptions::for_build(true)
    } else {
        OptOptions::default()
    };
    let mut core = compile_file_to_optimized(&src_path, &opts)
        .unwrap_or_else(|e| panic!("compile {rel_example}: {e}"));
    // Mono/opt clone append order is not semantically meaningful; stabilize for golden.
    core.functions.sort_by(|a, b| a.name.cmp(&b.name));
    let got = format_module(&core);
    let path = golden_path(stem);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir golden");
        }
        std::fs::write(&path, &got).expect("write golden");
        return;
    }
    let expect = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden {} ({e}); run with UPDATE_GOLDEN=1 to create",
            path.display()
        )
    });
    // Normalize in case checkout used CRLF (Windows autocrlf) despite .gitattributes.
    let expect = expect.replace("\r\n", "\n");
    assert_eq!(
        got, expect,
        "Core IR golden mismatch for {rel_example}\n--- got ---\n{got}\n--- expect ---\n{expect}"
    );
}

macro_rules! golden {
    ($name:ident, $path:expr) => {
        #[test]
        fn $name() {
            assert_core_golden($path, false);
        }
    };
    ($name:ident, $path:expr, release) => {
        #[test]
        fn $name() {
            assert_core_golden($path, true);
        }
    };
}

golden!(golden_tco_sum, "examples/tco_sum.lm");
golden!(golden_tco_even_odd, "examples/tco_even_odd.lm");
golden!(golden_tco_funref, "examples/tco_funref.lm");
golden!(golden_poly_id, "examples/poly_id.lm");
golden!(golden_poly_option_map, "examples/poly_option_map.lm");
golden!(golden_poly_top_dbl, "examples/poly_top_dbl.lm");
golden!(golden_trait_poly_method, "examples/trait_poly_method.lm");
golden!(golden_trait_poly_show, "examples/trait_poly_show.lm");
golden!(golden_small_list_local, "examples/small_list_local.lm");
golden!(golden_small_adt_local, "examples/small_adt_local.lm");
golden!(golden_pe_list_len_get, "examples/pe_list_len_get.lm");
golden!(golden_pe_map_contains, "examples/pe_map_contains.lm");
golden!(golden_pe_list_concat, "examples/pe_list_concat.lm");
golden!(golden_pe_map_get, "examples/pe_map_get.lm");
golden!(golden_par_map, "examples/par_map.lm");
golden!(golden_par_map_capture, "examples/par_map_capture.lm");
golden!(golden_memo_tf, "examples/memo_tf.lm", release);
golden!(golden_memo_local, "examples/memo_local.lm", release);
golden!(golden_escape_pure_len, "examples/escape_pure_len.lm");
golden!(golden_fuse_hof, "examples/fuse_hof.lm", release);
golden!(golden_small_map_local, "examples/small_map_local.lm");
golden!(golden_small_set_local, "examples/small_set_local.lm");
golden!(golden_for, "examples/for.lm");
golden!(golden_hof_float_apply, "examples/hof_float_apply.lm");
golden!(golden_float_map_keys, "examples/float_map_keys.lm");
golden!(golden_float_struct_eq, "examples/float_struct_eq.lm");
golden!(golden_eq_hash_consistent, "examples/eq_hash_consistent.lm");
golden!(golden_alt_option, "examples/alt_option.lm");
golden!(golden_alt_option_return, "examples/alt_option_return.lm");
golden!(golden_return_capture, "examples/return_capture.lm");
golden!(golden_range_map, "examples/range_map.lm");
golden!(golden_range_fold, "examples/range_fold.lm");
golden!(golden_take_escape, "examples/take_escape.lm");
golden!(golden_list_set, "examples/list_set.lm");
golden!(golden_list_set_alias, "examples/list_set_alias.lm");
golden!(golden_pe_adt_field, "examples/pe_adt_field.lm");
golden!(
    golden_par_map_toplevel_lam,
    "examples/par_map_toplevel_lam.lm"
);
golden!(golden_par_map_fn, "examples/par_map_fn.lm");
golden!(golden_float_ops, "examples/float_ops.lm");
// `import_as` needs multi-file load (CLI/`lumi::check_program`); not in Core pipeline.

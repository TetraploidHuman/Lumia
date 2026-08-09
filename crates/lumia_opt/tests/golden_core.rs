//! Core IR golden snapshots for high-signal examples.
//!
//! Regenerate with: `UPDATE_GOLDEN=1 cargo test -p lumia_opt --test golden_core`

use lumia_core::format_module;
use lumia_opt::{compile_file_to_optimized, OptOptions};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
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
golden!(golden_par_map, "examples/par_map.lm");
golden!(golden_par_map_capture, "examples/par_map_capture.lm");
golden!(golden_memo_l2, "examples/memo_l2.lm", release);
golden!(golden_memo_l0l1, "examples/memo_l0l1.lm", release);
golden!(golden_escape_pure_len, "examples/escape_pure_len.lm");
golden!(golden_fuse_hof, "examples/fuse_hof.lm", release);
golden!(golden_small_map_local, "examples/small_map_local.lm");
golden!(golden_small_set_local, "examples/small_set_local.lm");
golden!(golden_for, "examples/for.lm");

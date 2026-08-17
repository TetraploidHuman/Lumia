    use super::*;
    use lumia_opt::{compile_file_to_optimized, OptOptions};
    use std::path::PathBuf;

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    fn test_opts() -> CodegenOptions {
        CodegenOptions {
            release: false,
            output: PathBuf::from("/tmp/lumia_codegen_test"),
            emit_ir: false,
            runtime_lib: PathBuf::from("/tmp/unused_rt"),
            dense_f64_sr: true,
            link_args: vec![],
        }
    }

    fn emit_example(rel: &str, release: bool) -> String {
        let path = workspace_root().join(rel);
        let opts = if release {
            OptOptions::for_build(true)
        } else {
            OptOptions::default()
        };
        let core = compile_file_to_optimized(&path, &opts).expect("optimize");
        emit_verified_llvm_ir(&core, &test_opts()).expect("emit+verify")
    }

    #[test]
    fn emit_tco_sum_has_musttail() {
        let ir = emit_example("examples/tco_sum.lm", false);
        assert!(
            ir.contains("musttail") || ir.contains("tailcc") || ir.contains("tail "),
            "expected musttail-related IR in tco_sum; ir snip:\n{}",
            &ir[..ir.len().min(2000)]
        );
    }

    #[test]
    fn emit_memo_tf_has_lookup_and_store() {
        let ir = emit_example("examples/memo_tf.lm", true);
        // C ABI symbols stay `lumia_memo_l2_*` (frozen); planner name is `T_f`.
        assert!(
            ir.contains("lumia_memo_l2_lookup"),
            "expected lumia_memo_l2_lookup in memo_tf IR"
        );
        assert!(
            ir.contains("lumia_memo_l2_store"),
            "expected lumia_memo_l2_store in memo_tf IR"
        );
    }

    #[test]
    fn emit_hof_float_apply_keeps_f64_ret() {
        let ir = emit_example("examples/hof_float_apply.lm", false);
        assert!(
            ir.contains("dbl$Float") || ir.contains("apply$"),
            "expected mono Float/HOF clone names in IR; snip:\n{}",
            &ir[..ir.len().min(2500)]
        );
        // Float C ABI uses LLVM `double` for specialized / HOF-refined returns.
        assert!(
            ir.contains("double"),
            "expected f64/`double` ABI in hof_float_apply IR; snip:\n{}",
            &ir[..ir.len().min(2500)]
        );
    }

    #[test]
    fn emit_trait_poly_show_has_show_symbol() {
        let ir = emit_example("examples/trait_poly_show.lm", false);
        assert!(
            ir.contains("show") || ir.contains("Show") || ir.contains("__Show"),
            "expected Show-related symbol in trait_poly_show IR"
        );
    }

    #[test]
    fn emit_hello_verifies() {
        let _ir = emit_example("examples/hello.lm", false);
    }

    #[test]
    fn emit_float_map_keys_verifies() {
        let ir = emit_example("examples/float_map_keys.lm", false);
        assert!(
            ir.contains("lumia_ensure_map_f64") || ir.contains("lumia_map"),
            "expected float-map runtime symbols; snip:\n{}",
            &ir[..ir.len().min(2500)]
        );
    }

    #[test]
    fn emit_poly_option_map_verifies() {
        let _ir = emit_example("examples/poly_option_map.lm", false);
    }

    #[test]
    fn emit_par_map_verifies() {
        let ir = emit_example("examples/par_map.lm", false);
        assert!(
            ir.contains("lumia_list_par_map")
                || ir.contains("par_map")
                || ir.contains("ListParMap"),
            "expected parallel map-related IR; snip:\n{}",
            &ir[..ir.len().min(2500)]
        );
    }

    #[test]
    fn runtime_fn_missing_returns_err_not_panic() {
        let context = Context::create();
        let cg = Codegen::new(&context, "empty", false, true);
        let err = cg
            .runtime_fn("lumia_definitely_missing_symbol_zz")
            .expect_err("missing runtime symbol");
        let msg = err.to_string();
        assert!(
            msg.contains("missing runtime") || msg.contains("definitely_missing"),
            "unexpected: {msg}"
        );
    }

    #[test]
    fn codegen_error_display() {
        let e = CodegenError::msg("boom");
        assert_eq!(e.to_string(), "boom");
        let e2 = CodegenError::Llvm("bad".into());
        assert!(e2.to_string().contains("LLVM"));
    }

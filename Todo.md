# Lumia — 本轮未完成 / 待推进

记录经查实、尚未完整修复或需更大设计落地的问题。语义以 [docs/DESIGN.md](docs/DESIGN.md) 为准；分期见 [docs/BUILD.md](docs/BUILD.md)。
已确认落地的历史项已从本文件删除（见 git 历史）。

## 语义与运行时

- [ ] **Task/Channel 更大设计债**：ready_home/handoff/sweep 已落地；**`!Send` coro →** [`home_coro`](crates/lumia_rt/src/task/home_coro.rs)；**scan 指针 →** [`scan_ptrs`](crates/lumia_rt/src/task/scan_ptrs.rs)（`SchedCore` 自然 `Send`）。仍欠 **非 RT `Drop`（C-unwind ABI）**；堆 Mutex 下增量 mark 非真并行。

## 性能债（2026-08-15 审计确认）

仍欠的运行时/中后端性能（不重复架构卫生中的结构债）。

### 运行时热路径锁与探堆
- [ ] **`is_heap_payload` = 进程堆 Mutex + `heap_set` 查找**：`common.rs` `heap_gen`/`is_heap_payload`。COW 已对 List/ADT 信任 tid；**`eq` / `show` / `println_auto` / `hash` / `ord`（双标量）**与 **`value_rc_*_bits` / `remember_old_to_young` / `write_barrier` / `mark_value` / ADT `set_field` / Map overlay parent** 已用 `may_be_heap_payload_bits`（及 `is_heap_payload_bits`）跳过 Int/Bool/FunRef；**ADT float/bool mask sanitize 与 `list/core` promote/force 已加 `may_be_heap` 门**；**typed Show（list/set/map bool、list_adt、adt/adt_named）、`lumia_adt_eq`、`ord` 单边 immediate、`char_codepoint` 已改 `is_heap_payload_bits`**。真堆指针边仍每点探一次。
- [ ] **分配路径多次加锁**：**压力路径已改为** malloc 后单次 `with_heap`（insert 或 NeedCollect→collect→insert），去掉无 collect 时的 peek+insert 双锁；singleton 用 [`insert_young`](crates/lumia_rt/src/gc/alloc_ffi.rs)。仍欠 nursery bump / 延迟入 set。

### 并行与调度
- [x] **纤程栈 freelist**：scope 已走 `snapshot_scope_stack`/`recycle_scope_stack`；**已加** TLS [`take_fiber_stack`](crates/lumia_rt/src/task/scheduler.rs) / [`recycle_fiber_stack`](crates/lumia_rt/src/task/scheduler.rs) / [`recycle_coroutine_stack`](crates/lumia_rt/src/task/scheduler.rs)（`into_stack`；home-thread；取消与正常 Return 回收；未用 pre_stack 回池）。

### 中端优化缺口（相对 DESIGN §7.2）
- [ ] **融合仅 fold 汇合；缺 build 侧造林**：HIR 仅 `try_fuse_hof_fold`；`flatMap` 总 materialize；无 `Iota`/`Fused` 表示（DESIGN §7.3）。（`ConcatIdent` 过时注释已改准。）
- [ ] **Inline 仅体积阈值（≤32 ops）且仅 Release**：无热度；`IndirectCall` 不内联；捕获闭包 **恒堆分配**（escape 强制 `AllocClosure`）。
- [ ] **默认 Int `+/-/*` 走 `llvm.*.with.overflow`**：仅 `nsw_iv` 形标记免检；`nuw` 未用——一般循环付溢出分支，妨碍向量化。
- [ ] **堆类型 Let 默认 `root_push`**：缺通用 last-use 消根；AdtField→call 仍保守 retain。
- [ ] **通用 `List[Float]` 向量化靠 `dense_f64_sr` 整函数改写**：未匹配形状仍标量 SSA + RT list；SR 是特化逃生舱而非通用向量管线。
- [ ] **编译期：Memo plan ≈ O(n²)**（每候选扫全模块算 reuse）；escape 最多 32 轮×每函数不动点。

## 工具链

- [ ] **`--mm=arc` / 可插拔 GC 仍非优先**：分代 STW minor + 增量并发 full mark 已落地；ARC 后端仍为愿景。

## 架构卫生

下列为**仍欠**的结构/一致性债务。已确认落地的收口项已删除。

- [ ] **Core 堆/Float ABI 定型上帝模块**：`lambda_lift/float_abi.rs` 生产仍厚（**测已外置**）。**`prefer`/`join` 已迁出** [`value_ty/join.rs`](crates/lumia_core/src/value_ty/join.rs)。`local_heap_ty` 仍巨；与 `value_ty` / `mono/ret_ty` 仍欠整 walker 合流。
- [ ] **领域/基准 SR 侵入 codegen + RT**：`emit_value/{collatz,number_theory,trial_div,affine2,float}_sr.rs` 合计仍厚；RT 域核再挂特化 `#[no_mangle]`。**已抽**共享 [`sr_pattern`](crates/lumia_core/src/sr_pattern.rs)（codegen 薄再导出；opt `dense_f64_sr` 共用 peeps；**`float_sr` `header_lt_bound` 薄包**；**`has_float_approx`/`has_float_binop_with_const` 已迁 Core**；**`rem_eq_zero_operands`**；**`is_name_mul_const`**；**`header_gt_const`/`header_gt_eq` / collatz**；**`body_assigns_unit_inc`（trial_div）**；**`is_name_add_const` / `is_add_name_plus_name`（collatz）**；**`is_name_mul_const_plus_const` / `body_assigns_name_mul_const_plus_const`（collatz `3*x+1`）**；**`body_assigns_name_div_const`（collatz `/2`）**；**`name_ne_zero`（Euclid）**；**`is_add_name_plus_any` / `is_affine_ik1`/`is_affine_kj1`（number_theory）**；**`header_name_sq_le_name`（trial_div）** / **`header_name_sq_cmp`（nsw `square_bound`）**；**`is_unit_step`（nsw）**；**`is_name_ne_zero`（trial_div）**；**`is_name_rem_eq_const`/`is_name_div_const`/`rem_eq_zero_names`（collatz/trial_div）**）；**测已全部外置**；**测侧 `find_loops` 已统一**为 [`collect_loops`](crates/lumia_core/src/visit.rs)；**`first_direct_loop`**（number_theory / **affine2** 内层）；**`acc_add_rem_const_mod`（affine2 / number_theory acc+rem）**；**`add_name_other`/`rem_const_mod`/`body_assigns_rem`**（Euclid / get-affine）；**`is_name_mul_const`（affine2 Mul）**；**nsw `nonneg_iv` 已用 `header_gt_const`/`header_ge_const`**；**nsw `is_rem`/`is_name_mul_name`/`is_small_factor_mul_nonneg` 已迁 Core**；**trial_div then-arm 已用 `local_is_zero_or_false`**；**count-primes cond 已用 `local_let_matches` + 拒 clear-ok**；**nsw `collect_unit_counter`/`ge2` 已用 `collect_assigns`+`is_unit_inc`**。仍欠迁出 opt / feature-gate / 领域与语言 RT 分层。
- [ ] **`lumia_opt` `dense_f64_sr` 巨型单文件**：生产仍厚（**测已外置** [`dense_f64_sr_tests.rs`](crates/lumia_opt/src/dense_f64_sr_tests.rs)）。**已共用** Core [`sr_pattern`](crates/lumia_core/src/sr_pattern.rs)（含 nest peeps + **`collect_leaf_defs`** + **`same_local`/`is_unit_inc_value`** + **`is_nontrivial_add_or_sub`/`is_nontrivial_arith`**）；**nest Let 扫描已直调** [`for_each_let_value_ctrl`](crates/lumia_core/src/visit.rs)（删薄 `for_each_let`）；**薄 `collect_leaf_defs` 包装已删**（直调 Core）；**已 lock-in** `external_sig` ⊆ [`DENSE_F64_TRAMPOLINE_SYMS`](crates/lumia_abi/src/dense_f64.rs)（测 `trampoline_syms_have_external_sigs`；删死臂 sqrt/exp）。仍欠拆分 / feature-gate。
- [ ] **Core IR 穿透携带 `lumia_hir::Builtin`**：`Value::Builtin` 仍嵌 HIR 枚举（即便已有 `result_ty` stamp）→ `lumia_opt`/`lumia_codegen` 必须依赖 `lumia_hir`+`lumia_syntax`。前端改 builtin 强制中后端重匹配；中后端应只吃 Core 自有 opcode/元数据。
- [ ] **自动并行决策跨 HIR→ty 两阶段**：`list_hof` 在 lower 时先升为 `ListParMap`/`ListParFold`，`lumia_ty::finalize_auto_parallel` 再按 IO/非标量 demote 回顺序 desugar。并行策略散在前端两层，opt/codegen 只见结果；关并行或改安全条件需同时懂 HIR 启发式与 ty 回退。
- [ ] **跨层错误类型分裂**：syntax/hir `LocatedError`；ty `TypeError`；core/opt 管线大量 `Result<_, String>`；codegen 公开面以 `anyhow` 为主（另有未贯穿的 `CodegenError`）。诊断易丢 span、调用方无法统一处理。
- [ ] **库路径 panic vs Result 不一**：主路径多为 `Result`，但 `lumia_ty` infer/alt、`lumia_core` lower 等对「理论不可达」仍 `panic!`/`expect`（非 test）。宜统一为 ICE/`Err` 诊断。
- [ ] **双前端管线分叉**：`lumia_core::compile_source_to_core*`（单文件 parse→HIR→ty→Core，供单测）与 CLI/`check_program`（`load` 多文件 + `std.*` + visibility + assert 注解）并行。注释已承认差异；大量 core/opt 单测不经 loader，易漏 std/import/包路径回归。
- [ ] **`visit.rs` 未成为分析默认入口**：已有 `for_each_local_mut` / `for_each_block_dfs` / **`for_each_let_value`**（DFS Let 值；order-independent 用） / **`for_each_let_value_ctrl`**（仅 If/Loop，跳 Lambda；`dense_f64_sr` 共用） / **[`collect_ssa_live_refs`](crates/lumia_core/src/visit.rs)**（DCE 共用） / **[`first_direct_loop`](crates/lumia_core/src/visit.rs)** / **[`collect_loops`](crates/lumia_core/src/visit.rs)** / **[`local_let_matches`](crates/lumia_core/src/visit.rs)**（trial_div 等） / **[`collect_alloc_closure_caps`](crates/lumia_core/src/visit.rs)** / **[`collect_closure_cap_funrefs`](crates/lumia_core/src/visit.rs)**（Let 序；`directize`/`float_cap_fixup` 共用） / **[`collect_defined_locals`](crates/lumia_core/src/visit.rs)** / **[`collect_assigned_names`](crates/lumia_core/src/visit.rs)** / **[`collect_slot_names`](crates/lumia_core/src/visit.rs)** / **[`collect_leaf_defs`](crates/lumia_core/src/sr_pattern.rs)**；**`channel_hint`/`float_cap_fixup` AllocClosure**、**[`collect_float_cap_indices`](crates/lumia_core/src/visit.rs)** / **`float_abi::collect_fun_cap_tys_in_block`** / **[`collect_assigns`](crates/lumia_core/src/visit.rs)**（escape / **nsw unit·ge2 槽** 直调；薄包装已删）、**`float_cap_fixup` float-list/fold 扫描已改 `for_each_let_value`**、**[`collect_alloc_closure_env_funs`](crates/lumia_core/src/visit.rs)**（原 `mark_env_funs`） / **[`collect_call_names_in`](crates/lumia_core/src/visit.rs)**（traits stub）、**`block_calls`/`has_assign_or_name`**、**escape `collect_assigns`/`seed_escaping`**、**rewrite/inline/captures/LICM 共用 defined/assigned/slot 收集**、**opt `prefetch_inline_callees`**、**`mono/traits::collect_trait_method_refs`**、**`repr_select` If/Loop 块遍历** 已改 DFS/统一块 walk（`scan_block` / `mark_float` / bool·float_locals / `closure_cap_tys` / `collect_free` 仍按 Let 序；**`block_has_io` 故意不进 Lambda**；**`collect_slot_assigns` 故意不 DFS**——Assign fold/`prefer` 序敏感；**`repr_select` 故意不进 Lambda**；**`block_has_float_alloc_list` 故意仅顶层 Let**——勿改 DFS/`for_each_let_value*`）；多份 `*_sr` 生产匹配 / memo plan·CSE 仍手写嵌套 walker。**`captures` 测已外置** [`captures_tests.rs`](crates/lumia_core/src/lambda_lift/captures_tests.rs)。
- [ ] **Windows 工作流仍薄**：`scripts/env.ps1` 已前置 LLVM `bin` 到 PATH，并补 `LIBRARY_PATH`/`LIB`（缺时前置；与 `env.sh` 对称）。仍欠 Nix 级自动发现与完整 `.ps1` 工作流；README/BUILD 宣称 Linux+Windows，主路径仍是 Linux+Nix。
### 续（2026-08-15 第二轮；不重复上方条目）
- [ ] **Value→Type 三套完整并行 walker**：除已列的 join/prefer 近拷贝外，`value_ty` / `float_abi` / `mono/ret_ty` 仍各有重匹配。**已收口**：`merge_slot_ty`→`type_may_heap`；**`local_heap_ty` stamp-first**；**`join_abi_tys` + `prefer_concrete_heap_ty` 已迁** [`value_ty/join.rs`](crates/lumia_core/src/value_ty/join.rs)；**`join_fixed_ty` 已对齐** Float|{String,Bool,Char} / Fun vs scalar / **Fun×Fun** / List·Map·Set·Task·Channel / Adt 参数 prefer（测 `join_fixed_*`）；**多数 Builtin 经** [`builtin_value_ty`](crates/lumia_core/src/value_ty/builtin.rs)；**MapSet/Append/SetInsert/MapRemove** 共用 `resolve_heap_arg_ty`（仍保留相对 value_ty 的 float/`prefer` 特判；Append/SetInsert/MapRemove/`AdtField`/`**MapSet**` 已经 via+prefer，开放 MapSet 仍拒 Int-key→List）；**ChannelNew** / **ListConcat**（List×List 仍 `prefer`）/ **Range|RangeInclusive** 已汇入同一投影；**`mono/ret_ty`** 固定标量/Unit/String、**ListGet（seed）**、**gated Task·Channel join·recv·\*Opt（TaskJoin / ChannelRecv 已分闸）**、**Range\***、**Elems/MapKeys/MapValues/MapItems（gated）**、**ListTake 族（gated List）**、**TaskSpawn（gated Fun）**、**ChannelNew（stamp 或 via）**、**ListAppend/SetInsert/MapRemove（gated）**、**MapSet（仅 Map|List，拒开放 Int-key→List）** / **AdtField（`with_int_consts`）** / **ListConcat（双 List，无 prefer）** / **ListParMap（gated List）** / **ListParFold（acc）** / **AllocList|Set|Map / FunRef|AllocClosure / IndirectCall** 已 via `builtin_value_ty` 或固定投影。仍欠整 walker 合流（**ListPar* Float 特判仍在 float_abi**；**ListConcat prefer** 仍仅 float_abi；**Bool/Unit Builtin 判定**已 via `builtin_value_ty`（MatchFail 仍 bottom）；**`via_gated_recv`/`via_gated_recv_seeded`/`elems_family_recv_ok`** 已共用 Join·Recv·\*Opt / Elems 族 / **ListGet（float_abi 容器门；ret_ty 含 Option）** / ListTake 门（**float_abi 非 List → None**），以及 **Append/SetInsert/MapRemove/MapSet**（ret_ty seeded；**float_abi 已知 List/Map/Set 已 via+prefer**；**float_abi MapRemove 已知臂已 via+prefer**；**float_abi SetInsert 开放臂已 via**；开放 MapRemove 仍 via；开放 MapSet 仍拒 Int-key→List）/ **TaskSpawn** / **ListParMap（seeded；float_abi 有 List 时 via，Float 特判仍先）** / **ListParFold（acc 必有；有 List 时 via_gated_recv_seeded）** / **ListConcat（ret_ty 含 String 臂，无 prefer）** / **float_abi 固定标量·Unit·String Builtin（MatchFail 仍 bottom）**（MapSet 仍仅 Map|List）。
- [ ] **`ClosureCap.as_float` + `float_cap_fixup` 半吊子通道**：IR 上可变 `as_float` 旗标（`rewrite` 写入 → `float_cap_fixup` ≈1208 行事后补丁 → codegen `emit_calls` 消费），与 `param_tys`/`ret_ty`/闭包捕获表并行。**测已外置** [`float_cap_fixup_tests.rs`](crates/lumia_core/src/lambda_lift/float_cap_fixup_tests.rs)。**部分收口**：fixup 手建 ctx 走 [`InferValueCtx::with_fun_abi`](crates/lumia_core/src/value_ty/mod.rs)。Float 捕获 ABI 应只从 typed cap 表导出，删掉事后 mutation。体量/职责继续膨胀见第五轮。
- [ ] **`mono/specialize.rs` 上帝模块**：≈2135 行集 clone 发现、改写、ret refresh、forwarder 消除、FunRef HOF、Option/Result 载荷规则于一身；几乎每个 mono ABI 修复都落这里。宜按 collect / rewrite / ret_refresh / forwarders 拆分，并与 `ret_ty` 共享 lattice。
- [ ] **codegen `nsw_iv` 第二块基准形岛屿**：生产仍厚（Collatz/`3*x`、fib、matmul 形 peep；**测已外置** [`nsw_iv_tests.rs`](crates/lumia_codegen/src/nsw_iv_tests.rs)），经 `emit_fun` 焊进每个函数 emit。与已列 `*_sr` 同病但未收录——通用 NSW 被热核形状绑架。**已共用** [`name_of`](crates/lumia_core/src/sr_pattern.rs)/[`const_of`](crates/lumia_core/src/sr_pattern.rs)/[`is_unit_inc`](crates/lumia_core/src/sr_pattern.rs)/[`collect_assigns`](crates/lumia_core/src/visit.rs)/[`header_gt_const`](crates/lumia_core/src/sr_pattern.rs)/[`header_ge_const`](crates/lumia_core/src/sr_pattern.rs)/[`is_rem`](crates/lumia_core/src/sr_pattern.rs)/[`is_name_mul_name`](crates/lumia_core/src/sr_pattern.rs)/[`is_small_factor_mul_nonneg`](crates/lumia_core/src/sr_pattern.rs)/[`header_name_sq_cmp`](crates/lumia_core/src/sr_pattern.rs)（`square_bound`）；**薄 `collect_leaf_defs` 包装已删**（直调 Core）。宜迁 opt / feature-gate，codegen 只发 NSW 标记。
- [ ] **SSA `Local` + 字符串 `Name`/`Assign` 双寻址**：`Value::Name(String)` + `Op::Assign { name }` 与 SSA 并存；ABI/`slot_tys` 必须双轨跟踪。槽位应统一 `Local`/`SlotId`，名字仅调试打印。
- [ ] **`InferValueCtx` 可选表蔓延 / `FunIndex` 仅 mono**：`value_ty` 上下文堆 ≈8 个 `Option<&HashMap<…>>`；`fun_index` 仅 mono 用。**部分收口**：`ModuleTables` 供 mid-end/`closure_cap_tys`/`rewrite` 装配；codegen **`FunTables::seed_abi_from(ModuleTables)`** 已消 7 字段手抄（含 **`fun_param0_identity`** / **`trait_methods`**；`core_fun_is_param0_identity` 迁 Core；`closure_cap_tys` 读表；traits resolve 经 `ModuleTables`）。仍自建 LLVM 句柄与 `closure_cap_tys`/`adt_show_kinds`。
- [x] **Builtin→RT 符号已并入 `BuiltinInfo` 表字段**：[`BuiltinInfo`](crates/lumia_hir/src/builtin_info/mod.rs) 的 `string_receiver_rt` / `list_receiver_rt`（`with_*` 装配；`Builtin::*override` 读表；测 `receiver_rt_overrides_are_complete`）。codegen **`rt_symbol_for_*` 统一走** `Codegen::{string,list}_receiver_rt_override` 薄委托。
- [ ] **`lumia_ty` 分析未全走 HIR visit**：`free_vars`/`parallel`/`product_resolve`/`alt`/`traits` rewrite 已用 visit；**`effects` 已消双 walker**（删近拷贝 `assert_no_effects_in_pure`，纯函数只走一次 `check_expr_effects`；`fun_body_has_io` 仍因「不进 Lambda 体」保留专用 walk）。
- [x] **RT FFI 边界 crate 级放行「看似 safe」**：原 `lumia_rt` `#![allow(clippy::not_unsafe_ptr_arg_deref)]` **已删除**（`-D clippy::not_unsafe_ptr_arg_deref` 干净）。指针 C ABI 已按子系统标 `unsafe extern "C"` + 模块 `deny`（dict/dispatch/hash_ord/show/list/{core,ops,par,tid}/channel/fiber/alloc_ffi/map_set/memo/string_io/adt_show/frame_push/**dense_f64**/**cn_kernels**/**efe**；**`lumia_ptr_eq` 亦 `unsafe`**；**`eq` 已 path-aware + `deny`**）。**channel/fiber join·send·recv 已补 per-fn `# Safety`**。**`dispatch` len/concat/set/elems/remove/get/contains 已 path-aware `unsafe { crate::… }`**。**`map_ops`/`set`/`memo` lookup·store 已补 per-fn `# Safety`**。**`show` 死薄包装已删；`append_show_value`/typed show 已 path-aware**。**`list/ops`·`list/par`·`string_io/string` 已补 per-fn `# Safety`**。**`list/par` 已 path-aware 收窄 + `forbid_list_parallel`/`note_par_task_demotion` + 测 [`par_tests`](crates/lumia_rt/src/list/par_tests.rs) 已接线**。**`alloc_ffi` root/write_barrier + `io` println_str/cstr 已补 per-fn `# Safety`**。**`list/tid` ensure_f64 + fiber spawn/scope 已补 per-fn `# Safety`**。
- [ ] **CI/check 仍 `clippy`/`test --exclude lumia`**：`lumia`/`lumia_core` clippy 债未收；slim 冒烟（`CARGO_TARGET_DIR=target/slim-lsp`、`--no-default-features`）已接 `check.sh`/CI，勿回退。

### 续（2026-08-16 第三轮；不重复上方条目）
#### IR / 类型层
- [ ] **树形 Core 冒充 SSA，无 CFG**：`Value::{If,Loop,Lambda}` 嵌整块 `Block`；`Op` 仅 Let/Effect/Assign/Break/Continue/Return。无基本块图 → 每个中端 pass 自写嵌套 walker；控制与数据同 enum；`Break`/`Continue` 无 loop id，嵌套循环靠 codegen 约定。宜真 CFG（或明确「树 IR + 统一 visitor」并删伪 SSA 叙事）。
- [ ] **中端仍吃开放 `lumia_ty::Type`，无封闭 Core ABI 类型**：`CoreFun::{param_tys,ret_ty}` / channel hint / float_abi 继续用 `Type::Var` 与哨兵 `Var(u32::MAX)`。与已列 `List(Int)` 软占位正交——整条 ABI 合同是 HM 残留而非闭集 ABI。宜 lower 后收成 `CoreTy` lattice，opt/codegen 只认它。
- [ ] **效应三套真源**：`lumia_ty::Effect`（含 Var）、`BuiltinEffect`（Pure/Io）、`Op::Let.pure_region` 驱动 CSE/LICM/折叠；另有 `ty/effects.rs` 事后整树审计。opt 可按 `pure_region` CSE 而不机械绑定 `CoreFun.effect`/`BuiltinInfo`。宜单一效应 IR + 派生标记。
- [ ] **`Scheme` 假类型类袋**：已扩到 **9** 套平行 `*_vars`（`num`/`ord`/`eq`/`len`/`concat`/`contains`/`set`/`elems`/`take`）与真 `trait_preds` 并列。**已收口**开放方法 bind：`len`/`concat`/`contains`/`set`/`elems`/`take` 共用 [`check_open_receiver_bind`](crates/lumia_ty/src/traits.rs)；`num`/`ord`/`eq` 与 `trait_preds` 仍各自一套。宜统一谓词 IR（`Num(α)` / `HasLen(α)` …），删并行 `*_vars`。
- [ ] **`match` 在 typing 前擦成 If**：syntax 有 `Match`；HIR 无 Match 节点（`match_arms`→`If`+`AdtTag`/`MatchFail`）；穷尽性仍吃 `lumia_syntax::MatchArm`。ty 看不见模式；诊断无法挂在 typed Match 上。宜 HIR 保留 Pattern/Match，ty 后再降。
- [ ] **trait/instance 塌成字符串旁表**：HIR `Item` 仅 Fun/Val；trait 数据在 `Module` 映射 → `CoreModule.trait_methods` → `mono/traits` 再解析短名。无结构化 TraitDef；UFCS 改写与 mono stub 易脱节。
- [ ] **表面无类型 AST（注解/FFI 皆 `String`）**：syntax/HIR `ty: Option<String>` / `param_ann`；解析仍在 ty 的 `parse_type_name`（已支持 bracket/`Point`）。宜 syntax 产出 `TypeExpr`。
- [ ] **Span 死于 Core；`type_at` 线性戳表**：Core `Op`/`Value` 无 Span；诊断中后端多为无位置 `String`；`type_at_span` 倒序扫。宜 Core 带 `Span`/`NodeId`，或诊断只经 typed HIR。
- [ ] **`BuiltinInfo` 非类型规则真源**：info 管 arity/family/effect/emit；真实规则在 `ty/infer/builtins/**` 手写匹配。新 builtin = 元数据 + ty 臂（+ 常再改 ABI walker）。宜表驱动 typing 或从 info 生成。
- [ ] **结构化并发在 HIR lower 抹平**：`scope`/`spawn`→`ScopeEnter`/`TaskSpawn` 等 builtin；ty/opt 不见作用域括号，cancel 嵌套无法结构性校验。
- [ ] **HOF/`for` 大量预类型脱糖**（广于已列 auto-parallel 两阶段）：`list_hof`/`for_loops`/`hof_fuse`/`collections` 在 ty 前冻成循环/builtin；融合形状不可经类型回收。宜保留 HOF 形至 typed 后再降，或把融合推迟到 Core/opt。
- [ ] **积/和双声明、单一 `Type::Adt`**：HIR `adts`+`products`；ty 只有 `Adt` + `ProductState` 旁表。字段/`with`/Show 永特判。宜一种 ADT 模型（或积为无 tag 特化但仍统一）。
- [ ] **`CoreModule` 是分析黑板**：`hash_adts`/`trait_methods`/`channel_elem_*` 等在 lower 填充、lambda_lift 再改。元数据所有权与「何时权威」不清。宜不可变 `CoreModule` + 旁路 `AnalysisFacts`。

#### 中端 / codegen / RT
- [ ] **编译选项仍四散，无单一 `CompileOptions`**：中端/codegen 仍分 `OptOptions`/`CodegenOptions`。**已加** CLI/`build` 统一 [`CompileOptions`](crates/lumia/src/build.rs)（`opt()`/`codegen()`/`check_knobs()`；`check_file`/`build_file` 同入口；测 memo 门闩、link 合并、`check_knobs`）。无 codegen feature 时 check 仍直调 `check_program`。
- [ ] **C vs Runtime marshalling 表仍双份**：用户函数仍统一 i64；foreign 已由 `ForeignAbi` 驱动 declare。宜继续收成描述表。**已修**：复用 `declare_runtime` 已有符号时强制 Runtime 对象编组（修复 `std.string` `lumia_list_join` 等 LLVM verify 失败）。
- [ ] **`emit_fun` 函数发射上帝模块**：已拆 prologue + [`block`](crates/lumia_codegen/src/emit_fun/block.rs) / [`tco`](crates/lumia_codegen/src/emit_fun/tco.rs) / [`cow`](crates/lumia_codegen/src/emit_fun/cow.rs) / **[`let_bind`](crates/lumia_codegen/src/emit_fun/let_bind.rs)**（ephemeral 根消除 + bind）；`mod.rs` 编排 ≈204。宜再视需要外置 memo 编排细节。
- [ ] **`Value::Loop` 开放 SR try 链**：已收成 `emit_value_loop_with_srs` + 有序 `registry` 数组（13 个 `try_emit_*`）；新增 `*_sr` 往数组追加，勿再写开放 if/else。仍欠迁出 opt / feature-gate。
- [ ] **Task ↔ GC ↔ list-par 硬耦合**：fiber/channel 仍触 alloc。**已收窄**：`forbid_list_parallel`；`snapshot_sched_gc_roots`；**`with_rooted_payload`**；**`snapshot_scope_stack`/`recycle_scope_stack`**；纤程栈 freelist（TLS `take`/`recycle`/`recycle_coroutine_stack`）经 [`concurrency_policy`](crates/lumia_rt/src/concurrency_policy.rs) 旁的 scheduler 契约。宜继续抽 shade 算法边界。
- [ ] **`lumia_opt` 第三前端入口**：`compile_source_to_optimized*` 仍走 `compile_source_to_core*`（跳 loader/std）。**已标明 fixture-only** + 测 `compile_source_to_optimized_skips_loader_std`（`println as log` 无 loader 必败）。生产路径继续 `check_program` → `optimize`。

#### 工具链 / 文档 / 测试
- [ ] **import 整模块内联、无编译单元边界**：`filter_items` 为私有被调者保留整模块；load 合成扁平 `Module`。无增量编译、无库 ABI；菱形只靠 `(file,name)`。宜真正 CU / 导出摘要。
- [ ] **LSP 进程级 `Mutex<State>` + Full sync only**：分析仍串在一把锁；已支持 `workspace/configuration` pull + `didChangeConfiguration` push（`lumia.autoParallel`）。multi-root 仍缺。
- [ ] **LSP 功能测跳过 loader**：**已 lock-in**：hover/inlay/semantic/definition/symbols/completion/diagnostics/**format**（`format_imported_alias_via_loader_surface`；format 仍走严格 `parse_module`）的 `*_via_loader`。其它面仍可补。
- [ ] **IDE Run/Check 走 CLI shell，分析走进程内 `check_program`**：两套入口、两套 flag；无共享「工程构建」API。
- [ ] **正确性门四套并行**：e2e（全 CLI）、`opt_correctness`（共享 `tests/common` 的 `workspace_root`/`lumia_bin`）、`golden_core`（无 loader）、RT `task::stress`。loader/std/import bug 易漏 golden；`build_and_run` 输出路径/指纹仍分叉。宜一条「程序管线」测 + 分层夹具。
- [ ] **`bench_cn_*.sh` 近克隆骨架**：hot/step/efe/fuse/forward/strict 同构。**已抽** [`bench_measure_runs`](scripts/bench_measure.sh) / `bench_checksum_parity` / `bench_print_speedup_pair`；efe/step/fuse/forward/strict/**hot** 已瘦身；**vs_torch** 已用 `bench_measure_runs`（torch 路径保留）。

#### 补遗（同轮复核；不重复本轮已列）
- [ ] **`Type`/`Effect` 住在 `lumia_ty`，Core 硬依赖推断 crate**：`lumia_core`→`lumia_ty`；IR 直接嵌 `lumia_ty::{Type,Effect}`。与已列「收成 `CoreTy`」互补——即便有 `CoreTy`，抽 `lumia_types`（或 abi 旁路）才能让 opt/codegen 不绑 HM。宜类型定义与推断分 crate。
- [ ] **和类型 `sum_max_arity` 垫成统一 `params` 向量**：lower 算最大变体元数；ty/`value_ty`/`mono`/`AdtField` 按此垫 `Type::Adt.params`。这是上方「异变体载荷共享类型变量」的**表示根因**（Prelude Option/Result 靠字符串特判绕开）。宜 per-variant payload，勿 max-arity 积。
- [ ] **`lambda_lift` 名不副实，实为 ABI 厨房**：真 lift 仍在 `rewrite`/`captures`/`heap`。**已加** [`abi_refine`](crates/lumia_core/src/lambda_lift/abi_refine.rs) 门面（`channel_hint`/`float_cap_fixup`）；`mod.rs` 文档划界。`float_abi` 体量/迁目录仍欠。
- [ ] **`lower_hir` 编排中端遍，而非纯 HIR→Core**：**已抽出** [`run_core_abi_pipeline`](crates/lumia_core/src/lower/mod.rs)（lift→hint→directize→trait→fixup×mono→stubs）；`lower_hir_with_schemes` 在 prelude stub 后调用。宜最终迁出 lower crate/模块，lower 只做纯翻译。
- [ ] **Escape / Lit\* repr 所有权骑 core↔opt**：`escaping` 与 `*Repr` 在 core 定义并默认 `Heap*`；真正填充在 opt Escape/ReprSelect。opt 前 Core「合法但不完整」。宜 opt-only 注解或显式「after escape」阶段类型。

### 续（2026-08-16 第四轮；不重复上方条目）
#### 前端 / 类型 / 诊断
- [ ] **表面糖在 parser 抹平**：`a..b`/`a to b`/裸 `{ it }` 在 parse 成 Call/Lambda；syntax AST ≠ 书写面。**已修 fmt**：`rangeInclusive`/`range`/`to` → `..`/`..<`/` to `；裸 `bare_it` 回打成无 `it ->`；末参 Lambda/Block 回打尾随闭包（`f { … }` / `f(a) { … }`）。测 `fmt_range_and_to_surface_sugar` / `fmt_bare_it_surface_sugar`。LSP 不对发明的 bare `it` 做参数声明着色。仍欠 typed/HIR 脱糖阶段。**已修嵌套裸 `it`**：`expr_uses_ident` 对已绑定 `it` 的嵌套 Lambda 不再算自由使用；测 `nested_bare_it_lambda_*` / e2e `nested_it_map`。
- [ ] **仅 item 级恢复 + 列 0 同步启发**：`parse_module_recovering`/`synchronize_item`；无表达式级恢复。一处坏表达式可吞整项。
- [ ] **`bump` 每步 clone 带 String 的 Token**：无 intern/arena。解析所有权模型偏重。
- [ ] **一切积/和盲插 `Eq`/`Show` instance**：`collect_instances` 对所有 product/ADT（含 prelude）插入。派生策略非 langitem/注册表。

#### RT / opt / codegen
- [ ] **三套互不兼容的「持久更新」模型**：List/ADT 头 `rc` COW；Map Overlay（`count==-1`、无 RC）；Set 总是整表拷（命中 contains 仍 memcpy）。无共享持久容器层。
- [x] **Map/Set 开哈希近克隆**：**已抽**共享 [`open_hash_find_slot`](crates/lumia_rt/src/map_set/hash_probe.rs) / claim / from_linear / finish / [`compact_linear_entries`](crates/lumia_rt/src/map_set/hash_probe.rs)（stride + last_wins；map/set compact 薄包装）。持久更新模型（Overlay / 整表拷）仍见上方「三套互不兼容」条目。
- [ ] **nursery 仍非 bump**：注释已改为 young list；实现仍是 `alloc` + `h.young.push`，无 bump 区、无延迟入 set。
- [ ] **Memo 存 TLS、堆是进程全局**：`MEMO_TF` TLS + `MEMO_REGISTRY` 供 GC 扫；OS worker 间不共享命中。与 `PROCESS_HEAP` 不对称。
- [x] **Memo 规划无视 `IndirectCall`/FunRef**：`plan.rs` 跟踪 `FunRef(self)` SSA 别名，对解析到自身的 `IndirectCall` 施加与 `Call` 相同的 `param-k` 结构递归证明。测 `memo_tf_indirect_self_call_planned_dense` / `memo_tf_indirect_call_other_funref_not_self`。
- [ ] **Escape 摘要键为函数名字符串**：`Value::Call` 仍按名解析。**已收口摘要存储**：`EscapeSummaries` 按模块内 `EscapeFunId`（函数下标）存 `ParamEscape`，Call 经 `name → id` 查找；锁测 hit/miss 仍成立。宜 Call/FunRef 携带稳定 FunId，删掉名字解析。
- [ ] **目标三元组仍锁宿主**：`TargetMachine::get_default_triple()` / host CPU；仍欠交叉编译与「只出对象不链」。中间 `.o` 已默认删（`LUMIA_KEEP_OBJ` 保留）。
- [ ] **workspace Inkwell 钉死 `target-x86`**：非 x86 宿主结构性出局（即便 `initialize_all`）。
- [ ] **`lumia_rt`/`opt`/`core` 无 Cargo feature**：领域核/SIMD/stress 无法包级裁剪；静态库永远全量（与已列 SR 入侵互补——缺门闩）。
- [ ] **RT 测例半迁**：已有 `crate_tests/{eq,gc,list,…}`；**已外置** scheduler/fiber/channel 测 + [`hash_probe_tests`](crates/lumia_rt/src/map_set/hash_probe_tests.rs) + [`list/par_tests`](crates/lumia_rt/src/list/par_tests.rs) + **[`list/ops_tests`](crates/lumia_rt/src/list/ops_tests.rs)** + [`dispatch_tests`](crates/lumia_rt/src/dispatch_tests.rs) + [`eq_tests`](crates/lumia_rt/src/eq_tests.rs) / [`primes_tests`](crates/lumia_rt/src/primes_tests.rs) / [`dict_tests`](crates/lumia_rt/src/dict_tests.rs) / **[`dense_f64_tests`](crates/lumia_rt/src/dense_f64_tests.rs)** / **[`cn_kernels_tests`](crates/lumia_rt/src/cn_kernels_tests.rs)** / **[`efe_tests`](crates/lumia_rt/src/efe_tests.rs)** / **[`affine2_tests`](crates/lumia_rt/src/affine2_tests.rs)** / **[`float_kernels_tests`](crates/lumia_rt/src/float_kernels_tests.rs)** / **[`collatz_tests`](crates/lumia_rt/src/collatz_tests.rs)** / **[`number_theory_tests`](crates/lumia_rt/src/number_theory_tests.rs)** / **[`mutator_tests`](crates/lumia_rt/src/mutator_tests.rs)** / **[`f64_simd_tests`](crates/lumia_rt/src/f64_simd_tests.rs)** / **[`string_tests`](crates/lumia_rt/src/string_io/string_tests.rs)** / **[`heap_tests`](crates/lumia_rt/src/heap_tests.rs)** / **[`sched_core_tests`](crates/lumia_rt/src/task/sched_core_tests.rs)** / **[`globals_tests`](crates/lumia_rt/src/globals_tests.rs)** / **[`pressure_tests`](crates/lumia_rt/src/gc/pressure_tests.rs)** / **[`sched_env_tests`](crates/lumia_rt/src/task/sched_env_tests.rs)** / **[`limits_tests`](crates/lumia_rt/src/gc/limits_tests.rs)** / **[`stress_tests`](crates/lumia_rt/src/task/stress_tests.rs)** / **[`crate_tests/gc_helpers`](crates/lumia_rt/src/crate_tests/gc_helpers.rs)**（自 common 迁出 GC 测助手；删死 `is_old_header` 包装）。仍可再压 common 生产侧。
- [ ] **`examples/` 扁平回归堆**：≈244 顶层 `.lm` 混 `bad_*`/`bench_*`/`task_*`/教程；仅 `regress/` 成类。e2e 指进这锅汤。宜 `examples/{guide,reject,bench,task}`。

#### LSP / 包 / 编辑器 / CLI
- [ ] **LSP 诊断仍恒 Error，缺 relatedInformation/tags**：`severity` 已接 `DiagnosticKind::lsp_severity`。**已加** `DiagnosticKind::Warning`（severity=2）；包级 `trust_foreign_pure` 经 recovering check 发布 Warning（不失败 check/build；测 `package_trust_foreign_pure_emits_warning_not_error` / `warning_kind_maps_to_lsp_severity_two`）。仍欠 relatedInformation/tags 与更多 soft 诊断种类。
- [ ] **LSP 能力面缺口大**：无 references/rename/signatureHelp/codeAction/highlight/workspace symbol/call hierarchy/folding/cancel；不支持方法直接 `-32601`。`initialize` 忽略 client capabilities。
- [ ] **`pkg` 仍仅 init/lock/add**：缺 update/remove/outdated 等；`lumia run` 已落地（勿再记「无 run」）。
- [ ] **`bench_*` 本地 `measure_*`/`stats_*` 骨架近克隆**：已 `source bench_measure.sh` + 统一 release `lumia`；**cn 同构脚本（含 hot）与 vs_torch Lumia 侧**已用 `bench_measure_runs` / parity / speedup。

### 续（2026-08-16 第五轮；不重复上方条目）
- [ ] **`float_cap_fixup` 仍为第二 ABI 上帝模块（≈1208）**：**测已外置**；宜拆 `abi_refresh` 并冻结行数，删事后 `as_float` mutation 通道。
- [ ] **`CodegenTypeTables` 半收口、`ModuleTables` 扩展中**：入口已迁：fixup/channel_hint/rewrite/`closure_cap_tys`/FunTables 播种；**`ModuleTables` 已含** `hash_adts`/`adt_variant_names`/`sum_max_arity`/`channel_elem_*` + fun tys；codegen emit **`FunTables::seed_abi_from`** 单一播种（测 `from_module_indexes_ret_and_params`）。`channel_hint::prefer_payload_ty` 已删。`float_abi` 生产路径参数面 / `closure_cap_tys` 计算仍可继续迁。
- [ ] **`lambda_lift/heap.rs` 第四套「是否堆」启发式**：**已收口 Builtin 结果**走 [`ResultHeap`](crates/lumia_hir/src/builtin_info/mod.rs)（`Never`/`Always`/`Typed`）；**Typed 已对齐** codegen：有 `result_ty` 走 [`type_may_heap`](crates/lumia_core/src/value_ty/mod.rs)；lower **已 stamp** `ChannelRecv`/`TaskJoin` 地面载荷。未 stamp 的 recv/join 仍非堆（标量常见，避免软 `List(Int)`）。**测已外置** [`heap_tests.rs`](crates/lumia_core/src/lambda_lift/heap_tests.rs)。仍欠 typed 表驱动 + 与 float_abi/mono 合流。测 `lambda_lift::heap::tests::*` / `stamp_tests::channel_recv_list_payload_stamped_ground`。
- [ ] **`runtime_decls.rs` 手维百科 ≈1252 行**：在已列「与 `no_mangle` 不对账」之外——表本身成巨型单文件，每加 RT 导出就手工追加。宜从 `lumia_rt` 导出生成/diff，或按子系统拆表并强制 CI 对账。
- [ ] **`scheduler.rs` 假拆分后仍为第二上帝**：已抽 queue/cancel/roots/resume + 测外置；主文件 ≈TLS / scope+fiber stack freelist / FFI / 薄再导出（体量已降）。**`!Send` coro + scan 指针**已拆至 [`home_coro`](crates/lumia_rt/src/task/home_coro.rs) / [`scan_ptrs`](crates/lumia_rt/src/task/scan_ptrs.rs)。

### 续（2026-08-16 第六轮；不重复上方条目）
#### 命名协议 / 契约边界
- [ ] **HIR 脱糖合成名成第六套（+）命名协议**：`list_hof`/`collections`/`hof_fuse`/`for_loops` 生成 `__map_acc_*` / `__fmap_acc_*` / …；**已收口真源** [`desugar_slots`](crates/lumia_hir/src/desugar_slots.rs)（前缀表 + `is_scalar_fold_acc_slot`；emitters 与 `float_cap_fixup` 共用）。仍欠 `LocalKind`/`SlotRole`，禁止中端解析字符串。**已修**：`__flt_acc` 黑名单对齐；`float_outers` 不再把 `List→Float`；补 `__union/isect/diff_acc`；`spawn_filter_list_*` / `spawn_mut_list_float_acc_*`。
- [ ] **Inline 再引入 `$inl{tag}_` 槽名**：`opt/inline.rs` 重写可变槽为 `$inl…`；与 `$` mono / `$c_` 共用 `$` 命名空间。宜 inline 用 `Local` 重编号，勿改写字符串槽名。

#### 双轨 / 近拷贝 / 包边界撒谎
- [ ] **双轨函数特化：类型 mono（core）× 常量 specialize（opt）**：**已 lock-in 阶段分界**：opt `dual_track_specialization_stages_are_separated`（pipeline 无 mono 名；Release 两次/`Debug` 一次 `specialize_const`）；core `scheme_poly_top_level_dbl` 断言 `mono_of` + 无 `$c_`。宜统一 Specialization 框架。
- [ ] **未知类型普遍 `unwrap_or(Type::Int)` / `ground_open_vars: Var→Int`**：与已列 `List(Int)`「可能堆」软占位 **正交**——这里是「未知→标量 Int」，错误方向相反。宜显式 `CoreTy::Unknown`，禁止 Int 作缺省。
- [ ] **`FunTables` 成 codegen 侧第二块 Core 黑板**：**已**从 `ModuleTables` 播种 `fun_*` + `hash_adts`/`sum_max_arity`/`channel_elem_*`/`adt_variant_names`；仍自建 LLVM 句柄与 `closure_cap_tys`/`adt_show_kinds`。宜继续只读 `AnalysisFacts`/`ModuleTables`，FunTables 仅 LLVM 句柄。

#### 测试结构 / 死 API / 过宽 pub
- [ ] **mono 上帝模块近距测仍空**：`mono/mod.rs` 测已外置并再拆 [`tests/`](crates/lumia_core/src/mono/tests/)；**已补**同文件测：`ret_ty`/`key`/`directize`。`specialize`/`traits`/`rewrite` 生产侧仍偏厚。

#### 前端 / RT / 包装 / 文档
- [ ] **`std.linalg` 仍占语言标准库**：`cn`/`efe` 已迁 `extras/`，`linalg.lm` 仍几乎全是 `foreign`→`lumia_f64_*`，且在 `std_mod` 白名单。域模块迁出不完整。宜迁 `extras.linalg`（或等价），RT 域核走 feature。
- [ ] **RT `dispatch.rs` = 开放方法的运行时孪生**：`lumia_len`/`concat`/`set`/`elems`/… 按 `type_id` 分发，与 ty 的 `*_vars` 同族语义分属两处。宜单一能力表生成/对账。
- [ ] **前端巨型分发入口**：`infer_module_inner` / `hir/lower_expr` / `parse_primary` 各两百行级总 match（syntax `expr.rs` 整文件 ≈753）。新糖/项种类都挤同一臂。宜按族拆文件 + sugar 独立 pass。
- [ ] **LSP format 仍对缓冲二次 `parse_module`（严格 pretty）**：semantic 复用 recovering `Analysis.surface`。**已明确契约**：format 必须严格树，禁止复用 recovering AST；parse 失败 `Err`（测 `format_document_parse_error_is_err_not_empty`）。可选：在 `Analysis` 缓存严格树或与 CLI fmt 共享 helper。
- [ ] **工作区级 clippy allow 仍宽**：**已收窄**：根 `workspace.lints.clippy` 清空；`too_many_arguments`/`type_complexity`/`collapsible_match` 仅 crate 顶 `#![allow]` 于 `lumia_core`/`opt`/`codegen`/`ty`。`syntax`/`abi` 在 `-D warnings` 下干净。宜继续下沉到具体模块。

### 续（2026-08-16 第七轮；不重复上方条目）
#### 模块环 / mono·opt 内部
- [ ] **`traits`/`specialize` 用空 `Block` `mem::replace` 抽体再改写**：为迁就 `FunIndex` 生命周期付 O(函数数) 双缓冲。宜签名切片索引或 `FunId`/arena。

#### RT / CLI / 编辑器 / 文档 / 测试
- [ ] **RT 全局初始化三轨 + 「一次缓存」双模式**：**已加** [`globals`](crates/lumia_rt/src/globals.rs) 契约表；**`par_worker_count` / `fiber_stack_bytes` / `simd_f64_available` 已登记并实现于此**；**GC pressure atomics** 已登记（实现在 `gc/pressure.rs`）；**`LUMIA_GC_INCREMENTAL` 解析** [`parse_gc_incremental_env`](crates/lumia_rt/src/globals.rs)（`gc/limits` 消费）。**`SCHED_ENV` 已登记**（`task/sched_env.rs`）。**`TASK_RUNTIME_USED` 已登记**（`note_task_runtime_used` / `task_runtime_used_latched`；`scheduler` 薄消费）。**`PAR_TASK_DEMOTIONS` 已登记**（`note_par_task_demotion` / `par_task_demotions`；`list/par` 薄消费）。**`BEFORE_TRAP` 已登记**（`set_before_trap` / `before_trap_hook`；`common::trap_abort` / `task` 薄消费）。**进程单例/TLS 已表注**（`PROCESS_HEAP`/`SCHED`/`MEMO_REGISTRY`/`REGISTRY`/`ADT_SHOW`/`DICTS`/`PAR_WORKER`/`CALL_STACK`；另 **sched TLS** `CURRENT_FIBER`/`SCOPE_STACK*`/`FIBER_STACK_FREELIST`/`SCOPE_KIND_CACHE`、**mutator `ROOTS`**、**memo TLS** `MEMO_TF`/`MEMO_IDX`/`MEMO_REGISTRATION`、**测试锁** `SCHED_UNIT_TEST_LOCK`——所有权仍在原模块）。其它实现仍分散；宜继续把新全局只经此表登记。
- [ ] **编辑器门禁仍半边**：已对账 vscode `package.json`↔`package-lock`、打印版本三角、IDEA `until-build` 放宽；仍欠 IDEA 缩进/注释契约、非 Linux CI 跑编辑器门、矩阵测。
- [ ] **opt/mono 测试密度仍不均**：**测已外置** [`dce_tests`](crates/lumia_opt/src/dce_tests.rs) / [`repr_select_tests`](crates/lumia_opt/src/repr_select_tests.rs) / [`copy_elim_tests`](crates/lumia_opt/src/copy_elim_tests.rs) / [`concat_ident_tests`](crates/lumia_opt/src/concat_ident_tests.rs) / [`specialize_const_tests`](crates/lumia_opt/src/specialize_const_tests.rs)。**已补** `mono/directize`（[`directize_tests`](crates/lumia_core/src/mono/directize_tests.rs)）、`mono/key`、`mono/ret_ty`（[`ret_ty_tests`](crates/lumia_core/src/mono/ret_ty_tests.rs)）；codegen [`lib_tests`](crates/lumia_codegen/src/lib_tests.rs)；opt Memo 已认 FunRef+IndirectCall 自递归。其它 pass / specialize 等仍偏薄。

### 续（2026-08-16 第八轮；不重复上方条目）
#### codegen / abi
- [ ] **codegen `roots::type_may_heap` 成第五套「是否堆」**：**已薄封装** [`lumia_core::type_may_heap`](crates/lumia_core/src/value_ty/mod.rs)；`value_may_heap` 复用 `value_alloc_may_heap`+`ResultHeap`；**mono `merge_slot_ty` / lift `ResultHeap`** 已接入同一 lattice；**`local_heap_ty` 已 stamp-first**；**Builtin 投影已扩至** `ChannelRecv*`/`TaskJoin*`/`TaskSpawn`（见上条）；**`mono/ret_ty` 大量 Builtin 已 via（含 ChannelNew/Append/SetInsert/MapRemove/gated MapSet）**。仍欠其余 Builtin / `mono/ret_ty` 完全合流（`mono/key::type_is_heap_structure` 为 ABI 擦除容器专用，有意不含 String/Fun）。
- [ ] **`emit_value_if` 根状态入口仍整表 clone**：**已改** `rooted_slots: Rc<HashMap>` — if 入口 `Rc::clone`（O(1)），变异 `make_mut` COW；musttail 擦除后 O(1) 重挂检查点。仍可再压成显式 undo 栈（无 HashMap）。

### 续（2026-08-16 第十二轮；不重复上方条目）
#### 并发安全 / 测试分叉 / 假收口
- [x] **`SchedCore: Send` 覆盖内含 `!Send` 的 `Coroutine`**：**已拆完** — 栈在 [`home_coro`](crates/lumia_rt/src/task/home_coro.rs) TLS；GC 根/帧/handle/yielder 经 [`scan_ptrs`](crates/lumia_rt/src/task/scan_ptrs.rs) newtype；**`SchedCore` 自然 `Send`**（测 `sched_core_is_send_after_scan_ptr_newtypes`）。无 crate 级 `unsafe impl Send`。
- [ ] **根分析「未知→非堆」假收口**：`slot_may_heap` 未知→非堆；`roots.rs` 对未知 Call/`ret_ty` 仍 `unwrap_or(true)` **有意过度入根**（注释已写明：缺表时 ABI 已当 Int，宁可多余 root）。不宜翻成 `false`；真统一需显式 `Unknown` lattice。

#### 拆分后第二上帝 / 近拷贝 / 测沼
- [x] **`gc/` 压力原子归属**：已拆 shade/alloc_ffi/limits + **[`pressure`](crates/lumia_rt/src/gc/pressure.rs)**（`ALLOC_PRESSURE_FAST` / `FULL_MARKING_FAST`；`Heap::refresh_*` 薄委托；测 `refresh_sets_pressure_*`）。
- [ ] **codegen `emit_value/builtin/` 族拆分后第二上帝（合计≈1214）**：`mod.rs` 百科 + `show.rs`≈319 + `task.rs`≈363。**`ObjI64I64Ptr`/`ObjI64OptionTags` 已抽** `emit_obj_i64_i64_ptr` / `emit_obj_i64_option_tags`；仍可再压 ADT mask。
- [ ] **codegen Show 双文件近拷贝 + typed 特化符号汤**：**已统一** `classify_show_form` → `ShowForm`；**Show RT 符号表** [`SPECIALIZED_SHOW_RT`](crates/lumia_codegen/src/emit_value/builtin/show.rs)（容器 + float/bool/generic/adt/adt_named）+ `call_show_rt_ptr`；**Eq RT 符号表** [`SPECIALIZED_EQ_RT`](crates/lumia_codegen/src/emit_eq.rs)；**Println RT 符号表** [`SPECIALIZED_PRINTLN_RT`](crates/lumia_codegen/src/emit_value/builtin/show.rs)（含 **`lumia_println_unit`**，修 typed `println(())` 缺符号）；**Show/Eq/Ord override** 共用 [`call_trait_override`](crates/lumia_codegen/src/emit_eq.rs)。测 `specialized_show_rt_symbols_are_declared` / `specialized_eq_rt_symbols_are_declared` / `specialized_println_rt_symbols_are_declared`。仍可再压 ADT mask 与 `emit_by_convention`。**已修 RT**：嵌套 ADT Show。
- [ ] **`emit_arith` 算术厨房**：**已拆** [`emit_arith/{checked,ops}.rs`](crates/lumia_codegen/src/emit_value/emit_arith/)（溢出/NSW/div-rem vs binary/float/Ord/unary；同目录测）。仍可再压 Num ICE / 与 `nsw_iv` 边界。
- [ ] **`mono/tests.rs` 外置成测沼**：**已再拆** [`mono/tests/{key,specialize,containers,rewrite,directize}.rs`](crates/lumia_core/src/mono/tests/)；子模块近距测仍可补。
- [x] **`fiber.rs`↔`scheduler` 责任糊边界**：已抽 [`sched_fiber_api`](crates/lumia_rt/src/task/sched_fiber_api.rs)（spawn/join/scope 状态机单侧）；`fiber.rs`≈226 仅 FFI + trap + suspend/park + 薄 facade。scope freelist / rooted publish 仍走 `concurrency_policy`。**coro 存储**已再抽 [`home_coro`](crates/lumia_rt/src/task/home_coro.rs)。
- [ ] **opt `inline.rs` 近半测淹没**：**已外置** [`inline_tests.rs`](crates/lumia_opt/src/inline_tests.rs)（生产 ≈416 / 测 ≈380）。**`collect_defined` 薄包装已删**（直调 `collect_defined_locals`）。`$inl` 协议仍欠。

## 已落地记录

历史收口与功能完成项已从本文件移除；详见 git 历史与 [docs/BUILD.md](docs/BUILD.md)。


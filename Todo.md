# Lumia — 本轮未完成 / 待推进

记录经查实、尚未完整修复或需更大设计落地的问题。语义以 [docs/DESIGN.md](docs/DESIGN.md) 为准；分期见 [docs/BUILD.md](docs/BUILD.md)。
已确认落地的历史项已从本文件删除（见 git 历史）。

## 语义与运行时

- [x] **一等 `listOf`/`mapOf`/`setOf`**：`val lo = listOf` 曾降成 `Value::Name` → codegen「unbound mutable」。现为 `FunRef(__prelude_*)` + 空容器 stub；e2e `prelude_ctor_first_class`。（带参调用仍走 `listOf(…)` 特化臂；`Println` 保持 `Int→Unit`。）
- [ ] **`String` 字素簇（grapheme）索引仍欠**：标量/`toLower`/`toUpper`/byte_len 已落地；用户面按 grapheme 计数与切片仍未做。
- [ ] **Task/Channel 更大设计债**：ready_home/handoff/sweep 已落地。仍欠 **非 RT `Drop`（C-unwind ABI）**；堆 Mutex 下增量 mark 非真并行。

## 性能债（2026-08-15 审计确认）

仍欠的运行时/中后端性能（不重复架构卫生中的结构债）。

### 运行时热路径锁与探堆
- [ ] **`is_heap_payload` = 进程堆 Mutex + `heap_set` 查找**：`common.rs` `heap_gen`/`is_heap_payload`。COW 已对 List/ADT 信任 tid；**`eq` / `show` / `println_auto` / `hash` / `ord`（双标量）**与 **`value_rc_*_bits` / `remember_old_to_young` / `write_barrier` / `mark_value` / ADT `set_field` / Map overlay parent** 已用 `may_be_heap_payload_bits`（及 `is_heap_payload_bits`）跳过 Int/Bool/FunRef。真堆指针边仍每点探一次。
- [x] **GC `mark_value` 每边再抢锁**：`mark_value_on` / `mark_on` / `scan_fields_on` 在已持有的 `Heap` 上递归；`mark_quantum` 与 STW mark 波不再每子字 `with_heap`。根播种路径仍可走对外 `mark`/`mark_value`（可重入）。
- [x] **shadow-stack `root_push/pop` 每临时都抢堆锁**：TLS 根向量改为每 mutator `Mutex`；`push`/`pop`/`take`/`set` 不再 `with_heap`。GC 仍持堆锁后按 **heap→roots** 锁各 mutator。`lumia_root_push` 仅在 `full_marking_fast` 时 shade。
- [x] **Memo lookup 整段 `with_heap`**：`MEMO_TF`/`MEMO_IDX` 同为 TLS `Mutex`；lookup/store/stats 不抢堆锁；store 在释放 memo 锁后按 `full_marking_fast` shade（避免 heap↔memo 死锁）。GC walk：**heap→memo**。
- [ ] **分配路径多次加锁**：热路径已合并 inhibit+压力为一次 peek，并加 `ALLOC_PRESSURE_FAST`（无压力时跳过 peek）；`finish_alloc`/sweep/limit 下 `refresh_alloc_pressure_fast`。`finish_alloc` 仍单独持锁。宜继续单次 `with_heap` 覆盖插入；nursery bump 延迟入 set。

### 分配与慢路径
- [x] **`lumia_show` / 嵌套 show 多段 `String` 分配**：容器/ADT 经 `append_show_*` 写入单一 Rust 缓冲后再 `alloc_string`；嵌套不再每元素 `lumia_show` 堆分配。
- [x] **`map_get` 总堆分配 Option ADT**：miss 路径改为按 `none_tag` 永生单例（`RC_SHARED` + `Heap.option_none`）；hit 仍堆分配 `Some`。

### 并行与调度
- [x] **spawn 仍克隆 scope**：spawn 经 `snapshot_scope_stack` 复用 freelist 缓冲拷贝 TLS 栈；fiber 结束/`scrub`/`restore_host` 走 `recycle_scope_stack`。纤程栈 freelist 仍欠。

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

- [ ] **Core 堆/Float ABI 定型上帝模块**：`lambda_lift/float_abi.rs` ≈3583 行（生产+同文件测试；持续膨胀）；`local_heap_ty` 单函数超长穷举。同层并行 `value_ty::join_value_tys` / `float_abi::join_heap_tys` / `mono/ret_ty::join_fixed_ty` 三套合流近拷贝（Float 优先臂注释互相引用）；**codegen `prefer_cap_ty` 已改走公开 `prefer_concrete_heap_ty`**（第四套并入）。`List(Int)` 作「可能堆」软占位再靠 `prefer_concrete_*` 让位。宜继续收成单一 lattice / 表驱动 walker。
- [ ] **领域/基准 SR 侵入 codegen + RT**：`emit_value/{collatz,number_theory,trial_div,affine2,float}_sr.rs` 合计 ≈4000+ 行；RT `cn_kernels`/`efe`/`collatz`/`number_theory`/… 再挂一批特化 `#[no_mangle]`（crate 内合计 ≈174）。`name_of`/`is_unit_inc`/`const_of`/`header_lt_*` 等在多份 `*_sr` 复制且签名不完全一致。通用管道被基准形状绑架；应抽共享 pattern 原语，领域内核与语言运行时分层（或标为 optional/bench feature）。
- [ ] **`lumia_opt` `dense_f64_sr` 巨型单文件**：≈1918 行整函数 shape 匹配（codegen 双份匹配已消，见下「第 1 期」）。仍缺与其它 `*_sr` 共用的匹配原语；`"lumia_f64_*"` 字符串表继续膨胀时易再漂移。
- [ ] **Core IR 穿透携带 `lumia_hir::Builtin`**：`Value::Builtin` 仍嵌 HIR 枚举（即便已有 `result_ty` stamp）→ `lumia_opt`/`lumia_codegen` 必须依赖 `lumia_hir`+`lumia_syntax`。前端改 builtin 强制中后端重匹配；中后端应只吃 Core 自有 opcode/元数据。
- [ ] **foreign 类型面是扁平别名旁路**：`parse_type_name` 只认 `ListFloat`/`ListString` 等单标识符（`infer/module.rs`），无 `List[T]`/`Map[K,V]` 语法；`std/linalg` 与 `extras.cn|efe` 依赖此旁路。与语言表面类型语法分裂，扩展 FFI 只能继续堆别名。
- [ ] **`MonoKind` 无法键化 Task/Channel/Tuple**：`type_to_mono` 对上述（及 Unit/Var）走 `_ => None`；`args_mono_key` 失败则整站跳过克隆。**Fun 已可键化**（`MonoKind::Fun` / `FunRef`）；Task/Channel 仅出现在 `type_is_heap_structure` 恢复路径。多态若以 `Task[T]`/`Channel[T]`/Tuple 为实参，单态管线结构性盲区（与 Float ABI 补丁正交）。Fun 键化残留见第五轮 `unwrap_or(Int)` 污染。
- [ ] **自动并行决策跨 HIR→ty 两阶段**：`list_hof` 在 lower 时先升为 `ListParMap`/`ListParFold`，`lumia_ty::finalize_auto_parallel` 再按 IO/非标量 demote 回顺序 desugar。并行策略散在前端两层，opt/codegen 只见结果；关并行或改安全条件需同时懂 HIR 启发式与 ty 回退。
- [ ] **跨层错误类型分裂**：syntax/hir `LocatedError`；ty `TypeError`；core/opt 管线大量 `Result<_, String>`；codegen 公开面以 `anyhow` 为主（另有未贯穿的 `CodegenError`）。诊断易丢 span、调用方无法统一处理。
- [ ] **库路径 panic vs Result 不一**：主路径多为 `Result`，但 `lumia_ty` infer/alt、`lumia_core` lower 等对「理论不可达」仍 `panic!`/`expect`（非 test）。宜统一为 ICE/`Err` 诊断。
- [ ] **双前端管线分叉**：`lumia_core::compile_source_to_core*`（单文件 parse→HIR→ty→Core，供单测）与 CLI/`check_program`（`load` 多文件 + `std.*` + visibility + assert 注解）并行。注释已承认差异；大量 core/opt 单测不经 loader，易漏 std/import/包路径回归。
- [ ] **`visit.rs` 未成为分析默认入口**：已有 `for_each_local_mut` / `for_each_block_dfs` 等，但 `float_abi` / `channel_hint` / `closure_cap_tys` / 多份 `*_sr` / escape·memo 仍手写嵌套 walker。新 `Value` 臂易漏改；与上帝模块叠加放大维护面。
- [ ] **Windows 工作流仍薄**：已有 `scripts/env.ps1` stub；README/BUILD 宣称 Linux+Windows，完整 `.ps1` 工作流与本地 LLVM 路径仍不对等。
- [x] **`lumia_rt` 公共 API 过宽**：`lib.rs` / `list` / `map_set` / `string_io` 已改为显式 `pub use {…}`（C ABI 符号表）；内部 `pub(crate)` 助手不再经 glob 漏出。
### 续（2026-08-15 第二轮；不重复上方条目）
- [ ] **Value→Type 三套完整并行 walker**：除已列的 join/prefer 近拷贝外，`value_ty`（≈955，含 `infer_value_ty_ctx` + 拆出的 `builtin_value_ty`）、`float_abi::{local,block}_*_heap_ty`（`local_heap_ty` 单函数 ≈687）、`mono/ret_ty`（≈720）各自重匹配几乎全部 `Value`/`Builtin` 臂；ABI 补丁常需改三处。应收成单一 typed analysis API，heap/mono/codegen 作薄客户端。
- [ ] **`ClosureCap.as_float` + `float_cap_fixup` 半吊子通道**：IR 上可变 `as_float` 旗标（`rewrite` 写入 → `float_cap_fixup` ≈1231 行事后补丁 → codegen `emit_calls` 消费），与 `param_tys`/`ret_ty`/闭包捕获表并行。Float 捕获 ABI 应只从 typed cap 表导出，删掉事后 mutation。体量/职责继续膨胀见第五轮。
- [ ] **`mono/specialize.rs` 上帝模块**：≈2135 行集 clone 发现、改写、ret refresh、forwarder 消除、FunRef HOF、Option/Result 载荷规则于一身；几乎每个 mono ABI 修复都落这里。宜按 collect / rewrite / ret_refresh / forwarders 拆分，并与 `ret_ty` 共享 lattice。
- [ ] **codegen `nsw_iv` 第二块基准形岛屿**：`nsw_iv.rs` ≈1071 行（Collatz/`3*x`、fib、matmul 形 peep），经 `emit_fun` 焊进每个函数 emit。与已列 `*_sr` 同病但未收录——通用 NSW 被热核形状绑架。宜迁 opt / feature-gate，codegen 只发 NSW 标记。
- [x] **Core IR 嵌 `lumia_syntax::{BinOp,UnOp}`**：已收成 `CoreBinOp`/`CoreUnOp`（`ops.rs`）；lower 边界 `Into`；opt/codegen 改匹配 Core 枚举。中后端仍可能因其它表面依赖 `lumia_syntax`。
- [x] **Prelude `Option`/`Result` 靠字符串魔改**：`lumia_hir::langitem` 助手已覆盖 mono/ty/value_ty/emit_eq/float_abi/adt_classify/codegen task/`memo` helpers；channel_hint/mono/memo 测例比较亦改走 `is_option*`。
- [ ] **SSA `Local` + 字符串 `Name`/`Assign` 双寻址**：`Value::Name(String)` + `Op::Assign { name }` 与 SSA 并存；ABI/`slot_tys` 必须双轨跟踪。槽位应统一 `Local`/`SlotId`，名字仅调试打印。
- [ ] **`InferValueCtx` 可选表蔓延 / `FunIndex` 仅 mono**：`value_ty` 上下文堆 ≈8 个 `Option<&HashMap<…>>`；`fun_index` 仅 mono 用，而 float_abi/fixup/channel_hint/codegen 反复手拼 `fun_ret_tys`。缺共享 `ModuleTables` → 表装配拷贝。`CodegenTypeTables` 已存在但几乎只服务 codegen（半收口见第五轮）。
- [x] **Builtin→RT 符号在 `BuiltinInfo` 外覆盖**：`list_receiver_rt_override`（`ListLen`/`MapSet`/`ListGet`→`lumia_list_*`）与既有 `string_receiver_rt_override` 并列；仍非 BuiltinInfo 表内字段，但是唯一 emit 覆盖面。
- [x] **HIR `visit` 未被 `lumia_ty` 使用**：`free_vars` / `parallel` / `product_resolve` 已走 `all_free_vars` / `for_each_expr_mut`；`effects`/`alt`/`traits` 仍手写 walker。
- [ ] **RT FFI 边界 crate 级放行「看似 safe」**：`lumia_rt` `#![allow(clippy::not_unsafe_ptr_arg_deref)]`，大量 `extern "C"` 不以 `unsafe fn` 标出。指针契约在类型系统外；UB 审计难。宜收窄 allow、ABI 边用 `unsafe fn` + 薄安全包装。
- [ ] **CI/check 仍 exclude lumia + install slim 未测**：`check.sh` 已对齐 `llvm-dynamic`；双方仍 `clippy --exclude lumia`；`install.sh` slim-LSP 产物 CI 未测。
- [ ] **编辑器版本与 LSP 生命周期仍欠**：LSP `serverInfo.version` 已用 `CARGO_PKG_VERSION`；vscode/IDEA 版本漂移与 shutdown/`exit` 契约仍欠（对账脚本仍开放）。

### 续（2026-08-16 第三轮；不重复上方条目）
#### IR / 类型层
- [ ] **树形 Core 冒充 SSA，无 CFG**：`Value::{If,Loop,Lambda}` 嵌整块 `Block`；`Op` 仅 Let/Effect/Assign/Break/Continue/Return。无基本块图 → 每个中端 pass 自写嵌套 walker；控制与数据同 enum；`Break`/`Continue` 无 loop id，嵌套循环靠 codegen 约定。宜真 CFG（或明确「树 IR + 统一 visitor」并删伪 SSA 叙事）。
- [ ] **中端仍吃开放 `lumia_ty::Type`，无封闭 Core ABI 类型**：`CoreFun::{param_tys,ret_ty}` / channel hint / float_abi 继续用 `Type::Var` 与哨兵 `Var(u32::MAX)`。与已列 `List(Int)` 软占位正交——整条 ABI 合同是 HM 残留而非闭集 ABI。宜 lower 后收成 `CoreTy` lattice，opt/codegen 只认它。
- [ ] **效应三套真源**：`lumia_ty::Effect`（含 Var）、`BuiltinEffect`（Pure/Io）、`Op::Let.pure_region` 驱动 CSE/LICM/折叠；另有 `ty/effects.rs` 事后整树审计。opt 可按 `pure_region` CSE 而不机械绑定 `CoreFun.effect`/`BuiltinInfo`。宜单一效应 IR + 派生标记。
- [ ] **`Scheme` 假类型类袋**：已扩到 **9** 套平行 `*_vars`（`num`/`ord`/`eq`/`len`/`concat`/`contains`/`set`/`elems`/`take`）与真 `trait_preds` 并列；`unify`/`traits::check_*_bind` 每加一类开放方法就复制一套 HashSet 传播。宜统一谓词 IR（`Num(α)` / `HasLen(α)` …），删并行 `*_vars`。
- [ ] **`match` 在 typing 前擦成 If**：syntax 有 `Match`；HIR 无 Match 节点（`match_arms`→`If`+`AdtTag`/`MatchFail`）；穷尽性仍吃 `lumia_syntax::MatchArm`。ty 看不见模式；诊断无法挂在 typed Match 上。宜 HIR 保留 Pattern/Match，ty 后再降。
- [ ] **trait/instance 塌成字符串旁表**：HIR `Item` 仅 Fun/Val；trait 数据在 `Module` 映射 → `CoreModule.trait_methods` → `mono/traits` 再解析短名。无结构化 TraitDef；UFCS 改写与 mono stub 易脱节。
- [ ] **表面无类型 AST（注解/FFI 皆 `String`）**：syntax/HIR `ty: Option<String>` / `param_ann`；唯一解析在 `ty` 的 `parse_type_name`。比已列 foreign 扁平别名更广——`List[T]` 与 FFI 别名都只能在 ty 里发明解析。宜 syntax 产出 `TypeExpr`。
- [ ] **Span 死于 Core；`type_at` 线性戳表**：Core `Op`/`Value` 无 Span；诊断中后端多为无位置 `String`；`type_at_span` 倒序扫。宜 Core 带 `Span`/`NodeId`，或诊断只经 typed HIR。
- [ ] **`BuiltinInfo` 非类型规则真源**：info 管 arity/family/effect/emit；真实规则在 `ty/infer/builtins/**` 手写匹配。新 builtin = 元数据 + ty 臂（+ 常再改 ABI walker）。宜表驱动 typing 或从 info 生成。
- [ ] **结构化并发在 HIR lower 抹平**：`scope`/`spawn`→`ScopeEnter`/`TaskSpawn` 等 builtin；ty/opt 不见作用域括号，cancel 嵌套无法结构性校验。
- [ ] **HOF/`for` 大量预类型脱糖**（广于已列 auto-parallel 两阶段）：`list_hof`/`for_loops`/`hof_fuse`/`collections` 在 ty 前冻成循环/builtin；融合形状不可经类型回收。宜保留 HOF 形至 typed 后再降，或把融合推迟到 Core/opt。
- [ ] **积/和双声明、单一 `Type::Adt`**：HIR `adts`+`products`；ty 只有 `Adt` + `ProductState` 旁表。字段/`with`/Show 永特判。宜一种 ADT 模型（或积为无 tag 特化但仍统一）。
- [ ] **`CoreModule` 是分析黑板**：`hash_adts`/`trait_methods`/`channel_elem_*` 等在 lower 填充、lambda_lift 再改。元数据所有权与「何时权威」不清。宜不可变 `CoreModule` + 旁路 `AnalysisFacts`。

#### 中端 / codegen / RT
- [x] **编译选项四散 + Debug 仍跑 `DenseF64Sr`**：`OptOptions::Default.dense_f64_sr = false`；Debug 管线已去掉 `DenseF64Sr`（Release/`for_build`/CLI 仍开）。选项对象仍四散，见结构债。
- [ ] **C vs Runtime marshalling 表仍双份**：用户函数仍统一 i64；foreign 已由 `ForeignAbi` 驱动 declare。宜继续收成描述表。
- [ ] **`emit_fun` 函数发射上帝模块（≈833 行）**：帧/根/COW/memo/`dense_f64` 早退/NSW/TCO/`Op` 分发挤在 `emit_function`。宜按生命周期拆（prologue / body / epilogue / 特化出口）。
- [ ] **`Value::Loop` 开放 SR try 链**：`emit_value/mod.rs` 在通用 loop 前串 ≈12 个 `try_emit_*`（与已列 `*_sr` 文件同病，但缺注册表/插件面）。顺序与 fallthrough 隐式膨胀。宜 matcher 注册表或迁出 opt。
- [x] **TLS `BACKEND` 空壳罩进程 `Heap`**：已去掉 TLS `BACKEND`/`MmBackend`；`MarkSweep` 为 ZST，FFI 直接调 inherent 方法，状态只在进程 `Heap` Mutex。真每线程 nursery / `--mm=arc` 仍欠。
- [ ] **Task ↔ GC ↔ list-par 硬耦合**：GC shade 拉 `task::snapshot_sched_gc_roots`；fiber/channel 调 alloc/root；`list/par` 看 `task_runtime_active()`。三子系统无法独立演化。宜窄接口（根枚举 / 「禁并行」谓词）。锁序已在 crate 文档展开（CI 禁令仍欠）。
- [ ] **`lumia_opt` 第三前端入口**：`compile_source_to_optimized*` 再调 `compile_source_to_core*`（仍跳 loader/std）。在已列双管线外再添「像完整编译」的捷径。宜只测 Core IR fixture，或强制经 `check_program`。

#### 工具链 / 文档 / 测试
- [ ] **import 整模块内联、无编译单元边界**：`filter_items` 为私有被调者保留整模块；load 合成扁平 `Module`。无增量编译、无库 ABI；菱形只靠 `(file,name)`。宜真正 CU / 导出摘要。
- [ ] **LSP 进程级 `Mutex<State>` + Full sync only**：分析仍串在一把锁；已支持 `workspace/configuration` pull + `didChangeConfiguration` push（`lumia.autoParallel`）。multi-root 仍缺。
- [ ] **LSP 功能测跳过 loader**：hover/inlay/semantic 等多走 `check_source`；import/`std`/overlay 回归只能靠真人多文件。宜 loader fixture 测。
- [ ] **IDE Run/Check 走 CLI shell，分析走进程内 `check_program`**：两套入口、两套 flag；无共享「工程构建」API。
- [ ] **`install.sh` 双二进制靠 `/tmp` 拷贝舞**：先 slim 拷 `/tmp`，再编全量，wrapper 路由 `lsp`。竞态/脆弱（超出「CI 未测 slim」）。宜 cargo feature 两次 `--out-dir` 或 workspace 双 bin。
- [ ] **正确性门四套并行**：e2e（全 CLI）、`opt_correctness`（近克隆 harness）、`golden_core`（无 loader）、RT `task::stress`。loader/std/import bug 易漏 golden；harness 逻辑重复。宜一条「程序管线」测 + 分层夹具。
- [ ] **`bench_cn_*.sh` 近克隆骨架**：hot/step/efe/fuse/forward/strict 同构；维护随领域 bench 线性涨（结构债，不止「cn 进 std」）。宜共用 `bench_measure` 驱动。

#### 补遗（同轮复核；不重复本轮已列）
- [ ] **`Type`/`Effect` 住在 `lumia_ty`，Core 硬依赖推断 crate**：`lumia_core`→`lumia_ty`；IR 直接嵌 `lumia_ty::{Type,Effect}`。与已列「收成 `CoreTy`」互补——即便有 `CoreTy`，抽 `lumia_types`（或 abi 旁路）才能让 opt/codegen 不绑 HM。宜类型定义与推断分 crate。
- [ ] **和类型 `sum_max_arity` 垫成统一 `params` 向量**：lower 算最大变体元数；ty/`value_ty`/`mono`/`AdtField` 按此垫 `Type::Adt.params`。这是上方「异变体载荷共享类型变量」的**表示根因**（Prelude Option/Result 靠字符串特判绕开）。宜 per-variant payload，勿 max-arity 积。
- [ ] **`lambda_lift` 名不副实，实为 ABI 厨房**：目录以 `float_abi`/`channel_hint`/`float_cap_fixup` 为主（合计数千行），真 lift（`rewrite`/`captures`）反而少数；`mod.rs` 还 re-export hint/fixup。与已列上帝模块正交——是**包边界撒谎**。宜拆 `lift` vs `abi_refine`。
- [ ] **`lower_hir` 编排中端遍，而非纯 HIR→Core**：末尾串 lift→hint→directize→trait→≤8×(fixup+mono)→stubs（`MAX_FLOAT_MONO_ROUNDS`）。与已列「魔法迭代上界」同管线、但是**所有权**债。宜 lower 纯翻译；具名 Core pass 管道 + 阶段不变量。
- [ ] **Escape / Lit\* repr 所有权骑 core↔opt**：`escaping` 与 `*Repr` 在 core 定义并默认 `Heap*`；真正填充在 opt Escape/ReprSelect。opt 前 Core「合法但不完整」。宜 opt-only 注解或显式「after escape」阶段类型。

### 续（2026-08-16 第四轮；不重复上方条目）
#### 前端 / 类型 / 诊断
- [ ] **互递归多态随声明序**：`infer_module_inner` 先绑 mono 占位，再按 item 序 generalize/`bind_scheme`。靠前函数见 mono 占位、靠后见 scheme——经典 HM 债，无 SCC 不动点文档/实现。
- [ ] **Span 键 rewrite/事实表会撞**：`ufcs_rewrites`/`alt_kinds`/字段/`with` 均 `HashMap<Span,_>`。同 span 静默覆盖；宜 `NodeId`。
- [ ] **表面糖在 parser 抹平**：`a..b`/`a to b`/裸 `{ it }` 在 parse 成 Call/Lambda；syntax AST ≠ 书写面；fmt/IDE「原样」丢失。宜 typed/HIR 脱糖阶段。
- [ ] **仅 item 级恢复 + 列 0 同步启发**：`parse_module_recovering`/`synchronize_item`；无表达式级恢复。一处坏表达式可吞整项。
- [ ] **`bump` 每步 clone 带 String 的 Token**：无 intern/arena。解析所有权模型偏重。
- [ ] **Lower 错误 `RefCell` 先错即终**：`set_err` 仅在空时写入；嵌套失败丢弃。无法多诊断 lower。
- [ ] **双类型打印机 + unify 行话**：`Display for Type`→`?N`；IDE `display_type` 接地+字母名；unify 发 `infinite type` / `Debug` mismatch。违 DESIGN §3.2 用户面措辞。
- [ ] **`join` 按元数重载 Task vs List**：`from_method` `(join,1)→TaskJoin`、`(join,2)→ListJoin`。同名两 builtin，易误解析。
- [x] **`show_methods` 仅 Show 旁路表**：已删；Show 仅经 `trait_methods[("T","show")]` / `mangle_trait_method`（codegen 本就如此）。
- [ ] **一切积/和盲插 `Eq`/`Show` instance**：`collect_instances` 对所有 product/ADT（含 prelude）插入。派生策略非 langitem/注册表。

#### RT / opt / codegen
- [ ] **三套互不兼容的「持久更新」模型**：List/ADT 头 `rc` COW；Map Overlay（`count==-1`、无 RC）；Set 总是整表拷（命中 contains 仍 memcpy）。无共享持久容器层。
- [ ] **Map/Set 开哈希近克隆**：`MAP_ST_*`/`SET_ST_*`、`*_hash_find_slot`/`*_from_linear_to_hash`/`*_finish` 平行拷贝。宜参数化一张表实现。
- [ ] **nursery 仍非 bump**：注释已改为 young list；实现仍是 `alloc` + `h.young.push`，无 bump 区、无延迟入 set。
- [ ] **Memo 存 TLS、堆是进程全局**：`MEMO_TF` TLS + `MEMO_REGISTRY` 供 GC 扫；OS worker 间不共享命中。与 `PROCESS_HEAP` 不对称。
- [ ] **Memo 规划无视 `IndirectCall`/FunRef**：`plan.rs` 只认 `Value::Call{fun:name}`。HOF 站点永不进 Slots——相对 FunRef ABI 栈结构性盲。
- [ ] **Escape 摘要键为函数名字符串**：`HashMap<String, ParamEscape>`；mono/`$c_` 改名是静默摘要键风险（与已列 Fun 字符串协议互补）。
- [ ] **目标三元组锁宿主；`.o` 留在产物旁**：`compile_module` 默认 triple+宿主 CPU，写出 `.o`/`.obj` 后 clang 链接且不删；无「只出对象不链」。根目录探针噪音的又一来源。
- [ ] **workspace Inkwell 钉死 `target-x86`**：非 x86 宿主结构性出局（即便 `initialize_all`）。
- [ ] **`lumia_rt`/`opt`/`core` 无 Cargo feature**：领域核/SIMD/stress 无法包级裁剪；静态库永远全量（与已列 SR 入侵互补——缺门闩）。
- [ ] **RT 测例半迁**：已有 `crate_tests/{eq,gc,list,…}`，大量 `#[cfg(test)]` 仍嵌生产文件（同 channel_hint/scheduler 淹没模式，RT 内未完成拆分）。
- [ ] **`env.sh` 版本钉死 Nix LLVM 21.1.8 store glob**：所有 check/e2e/install/bench source 它；非 Nix 仍扩 `/nix/store/*`；升 LLVM 必改钉（在「Windows vs bash」之上的版本耦合）。
- [ ] **`examples/` 扁平回归堆**：≈244 顶层 `.lm` 混 `bad_*`/`bench_*`/`task_*`/教程；仅 `regress/` 成类。e2e 指进这锅汤。宜 `examples/{guide,reject,bench,task}`。

#### LSP / 包 / 编辑器 / CLI
- [ ] **按 URI「当入口」改变可见性**：单独打开库文件 → `entry_file`=它；作为 import 则否。同文件诊断/hover ≠ 真入口包检查。
- [ ] **overlay 键经 canonicalize，loader `get` 路径身份脆弱**：符号链接/未规范化入口/未保存路径可 miss overlay。
- [ ] **LSP 诊断仍恒 Error，缺 relatedInformation/tags**：`severity` 已经 `DiagnosticKind::lsp_severity`（当前种类皆 Error=1）；`code` 已有。relatedInformation/tags 与 Warning 类诊断仍欠。
- [ ] **多文件 fail-fast 单诊断 vs 恢复路径多诊断**：CLI/LSP 多文件 `typecheck_hir`；缓冲恢复 `typecheck_hir_recovering`。体验分裂。
- [ ] **LSP 能力面缺口大**：无 references/rename/signatureHelp/codeAction/highlight/workspace symbol/call hierarchy/folding/cancel；不支持方法直接 `-32601`。`initialize` 忽略 client capabilities。
- [ ] **无 `lumia run`；`pkg` 仅 init/lock/add**：BUILD 能力表与 CLI 表面仍不齐（`fmt` 零文件已改为报错退出）。
- [ ] **IDEA liveTemplates 是第三套片段**（不经 shared→vscode 对账）。与已列关键字多源同病、片段面。
- [ ] **bench 测量骨架在 `bench_cn_*` 外仍克隆**：`bench_cn_vs_torch`/`bench_memo`/`bench_cpu` 本地 `measure_*`/`stats_*`；`bench_memo` 用 `target/debug/lumia`、其它偏 release。宜一律走 `bench_measure.sh`。

### 续（2026-08-16 第五轮；不重复上方条目）
- [ ] **`float_cap_fixup` 膨胀为第二 ABI 上帝模块（≈1231）**：远超原「`as_float` 半吊子通道」叙事——已吞 `refresh_lifted_lambda_rets` / `refresh_alloc_closure_fun_rets` / `upgrade_captured_list_fold_float` / call-site List 升级等；**零 `#[cfg(test)]`**。宜拆 `abi_refresh` 并入 typed cap 表，或外置测例并冻结行数。
- [ ] **`CodegenTypeTables` 半收口、`ModuleTables` 仍不存在**：「架构清理」已记 `CodegenTypeTables`，但生产路径几乎只在 codegen `emit_fun/helpers` + `closure_cap_tys` 使用；`float_abi`/`float_cap_fixup`/`channel_hint` 仍每遍手拼 `fun_ret_tys`（fixup 内多次重建）。代码中无 `ModuleTables` 符号——是未完成迁移。宜共享模块表 API，或删掉仅语法糖的包装以免叙事超卖。
- [ ] **`lambda_lift/heap.rs` 第四套「是否堆」启发式**：lift 用 `block_result_may_heap_with_params`（Builtin 白名单跳过 `ChannelRecv`/`TaskJoin` 等），与已列 `value_ty` / `float_abi` heap_ty / mono `fixed_ty` 并行。新 Builtin 易漏。宜并入单一 heap lattice，lift 只读 `ret_ty`/policy。
- [ ] **`runtime_decls.rs` 手维百科 ≈1064 行**：在已列「与 `no_mangle` 不对账」之外——表本身成巨型单文件，每加 RT 导出就手工追加。宜从 `lumia_rt` 导出生成/diff，或按子系统拆表并强制 CI 对账。
- [ ] **`scheduler.rs` 生产面膨胀（≈1254）**：不再被测试淹没（≈326 测试），但调度/亲和/env/GC 根快照仍挤同一文件；与已列 Task↔GC↔par 耦合叠加。宜按 queue/affinity/pool/roots 拆模块。

### 续（2026-08-16 第六轮；不重复上方条目）
#### 命名协议 / 契约边界
- [ ] **HIR 脱糖合成名成第六套（+）命名协议**：`list_hof`/`collections`/`hof_fuse`/`for_loops` 生成 `__map_acc_*` / `__fmap_acc_*` / `__tolist_acc_*` / `__fuse_acc_*` / `__fold_x_*` / `__i_*`；`float_cap_fixup` 用 `starts_with` 白/黑名单、`channel_hint` 用 `contains("__map_acc")` 消费。改脱糖前缀会静默改 ABI。与已列 `__lam_`/`__val_` 同族但未收录。宜 `LocalKind`/`SlotRole`，禁止中端解析 `__*_acc` 字符串。
- [ ] **Inline 再引入 `$inl{tag}_` 槽名**：`opt/inline.rs` 重写可变槽为 `$inl…`；与 `$` mono / `$c_` 共用 `$` 命名空间。宜 inline 用 `Local` 重编号，勿改写字符串槽名。

#### 双轨 / 近拷贝 / 包边界撒谎
- [ ] **双轨函数特化：类型 mono（core）× 常量 specialize（opt）**：`mono/specialize`（`$Float`/`MonoKey`）与 `SpecializeConstPass`（`$c_`）两套 clone/改写 Call；Release 交错顺序靠注释。宜统一 Specialization 框架，或阶段不变量测例。
- [ ] **未知类型普遍 `unwrap_or(Type::Int)` / `ground_open_vars: Var→Int`**：与已列 `List(Int)`「可能堆」软占位 **正交**——这里是「未知→标量 Int」，错误方向相反。宜显式 `CoreTy::Unknown`，禁止 Int 作缺省。
- [ ] **`FunTables` 成 codegen 侧第二块 Core 黑板**：镜像 `hash_adts`/`sum_max_arity`/`channel_elem_*`/`adt_variant_names` 并自建 `fun_ret_tys`/`closure_cap_tys`。权威在 CoreModule vs FunTables 不清。宜只读 `AnalysisFacts`/`ModuleTables`，FunTables 仅 LLVM 句柄。

#### 测试结构 / 死 API / 过宽 pub
- [ ] **mono 上帝模块近距测仍空**：`mono/mod.rs` 测已外置 `mono/tests.rs`；`specialize`/`float_cap_fixup`/`ret_ty`/`key`/`traits`/`rewrite` 同文件测仍为 0。宜按子模块外置测 + 行数预算。
- [x] **Core lower 残留 `.expect`**：Alt/With 与 `call`/`control` Unit 操作数/`if` 条件已 `note_ice`+`None`。
- [ ] **锁序缺 CI 禁令**：`lumia_rt` crate 文档已扩全表（heap→sched→roots/memo；channel/memo shade；DICTS/ADT_SHOW 独立）。CI 禁令仍欠。
- [x] **锁序文档未覆盖 memo/dict/mutator/channel**：已写入 `lumia_rt` crate `# Lock order`（见上条 CI 欠项）。

#### 前端 / RT / 包装 / 文档
- [ ] **`std.linalg` 仍占语言标准库**：`cn`/`efe` 已迁 `extras/`，`linalg.lm` 仍几乎全是 `foreign`→`lumia_f64_*`，且在 `std_mod` 白名单。域模块迁出不完整。宜迁 `extras.linalg`（或等价），RT 域核走 feature。
- [ ] **RT `dispatch.rs` = 开放方法的运行时孪生**：`lumia_len`/`concat`/`set`/`elems`/… 按 `type_id` 分发，与 ty 的 `*_vars` 同族语义分属两处。宜单一能力表生成/对账。
- [x] **`string_io.rs` 混装 String / IO / stdin / trap**：已拆 `string_io/{string,io,trap}.rs`；crate 仍 `pub use string_io::*`。
- [ ] **前端巨型分发入口**：`infer_module_inner` / `hir/lower_expr` / `parse_primary` 各两百行级总 match（syntax `expr.rs` 整文件 ≈753）。新糖/项种类都挤同一臂。宜按族拆文件 + sugar 独立 pass。
- [ ] **LSP semanticTokens（及 format）对已分析缓冲二次 `parse_module_*`**：`Analysis` 不缓存 syntax AST；着色走未 rewrite 表面树、靠 span 对 typed。与「typed HIR 权威」分裂叠加。宜缓存 AST 或明确「着色只认表面」。
- [ ] **工作区级 clippy allow 仍宽**：空 `codegen/src/bin/` 已删；根 `too_many_arguments`/`type_complexity`/`collapsible_match` 等仍 `allow`，掩盖上帝函数。宜收窄到具体模块。
- [ ] **`opt_correctness` 与 e2e harness 近克隆**：各自 `workspace_root`/`lumia_bin`/`build_and_run`。坐实已列「四套正确性门」的骨架分叉。宜抽 `tests/common`。

### 续（2026-08-16 第七轮；不重复上方条目）
#### IR / 表示半废弃
- [ ] **`MapRepr::SmallMap` 语义仍弱**：`repr_select` 不再选 LitMap/LitSet，空容器发 null；**SmallMap 仍被选出**，emit 对非 Assoc 基本一律 heap+finish。宜删 SmallMap 或实现真小表路径。

#### 模块环 / mono·opt 内部
- [ ] **`ir.rs` ↔ `visit.rs` 包内环依赖**：`ir` 调 `visit::{max_local_in_value,rewrite_value_locals}`，`visit` 再吃 `ir` 类型。难抽 `lumia_ir`。宜 remap/`max_local` 迁入 `ir` 或独立 `ir_ops`。
- [ ] **Inline / SpecializeConst 热路径整表 `CoreFun` 深拷**：候选表 `.map(|f| (name, f.clone()))`。异于 Memo O(n²)。宜索引借用，命中再克隆 body。
- [ ] **`traits`/`specialize` 用空 `Block` `mem::replace` 抽体再改写**：为迁就 `FunIndex` 生命周期付 O(函数数) 双缓冲。宜签名切片索引或 `FunId`/arena。

#### RT / CLI / 编辑器 / 文档 / 测试
- [ ] **RT 全局初始化三轨 + 「一次缓存」双模式**：`OnceLock`（heap/sched/memo/mutator/par）vs `Once`（trap hook/pool）vs 裸 `Mutex::new`（`ADT_SHOW`/`DICTS`）；`par_worker_count` 用 `OnceLock`，`simd_f64` 用 `AtomicU8+Relaxed`。无统一 lazy/缓存惯例，亦无 Atomic Ordering 契约表。宜 `rt::globals` + 文档化 Ordering。
- [ ] **编辑器门禁半边 + 版本三角分裂**：CI 仅 Linux 跑 `check_editor_assets`；脚本不管 IDEA 缩进/注释契约；vscode `0.3.9` / IDEA `0.3.0` / workspace `0.1.0` 无对账；IDEA `until-build="262.*"` 钉死单大版本。宜扩展对账脚本 + 文档「编辑器版本 ≠ 语言版本」+ 放宽/矩阵测 IDEA。
- [ ] **opt/mono 测试密度仍不均**：`copy_elim`/`repr_select` 已有最低 fixture；其它 pass / mono 子模块覆盖仍偏薄。宜每 pass 最低 fixture。
- [x] **`crate_tests` 仍无 task；e2e 宏海未拆**：已有 `crate_tests/task` + Core golden task；e2e 已拆 `task.rs`（`basic.rs` 不再混装 Task）。

### 续（2026-08-16 第八轮；不重复上方条目）

对照源码再挖 syntax/诊断坐标系、HIR↔ty、codegen 根/memo/mask、LSP 假绿、abi 杂仓；下列为仍欠项（本轮审计时曾写入，清理已勾选时补回）。

#### 位置协议 / 词法 / 诊断面

- [ ] **LSP 列按字节，协议默认 UTF-16**：`cursor::pos_to_byte` 把 `character` 当 byte 加；`initialize` 不协商 `positionEncoding`；测例全 ASCII。非 ASCII 源上 hover/补全/诊断/着色全偏。宜协商 UTF-8 或统一 UTF-16↔byte；加多字节金样。
- [ ] **CLI caret 列亦为字节，与「标量」叙事分裂**：`byte_to_line_col` 注释写 byte；DESIGN 用户面按 Unicode 标量。宜用户面列用标量（或文档钉死 byte）并与 LSP 共用一表。
- [x] **Parser 用户错误用 `TokenKind` 的 `Debug`**：`Display for TokenKind`；`expect` / `expect_rbrace` / expr 意外 token 改用 `{}`。期望集合文案仍可再润色。
- [ ] **恢复路径注入恒等毒桩 lambda**：`parse_val_item_resilient` 失败体换成 `{ _1,_2 -> _1 }` 类 stub 仍进符号表/定型。宜 `Hole`/`Error` 类型，或恢复项不绑定 scheme。
- [ ] **`peek_kinds` 旁路重词法 + Ident 再分配**：临时 `Lexer` 调 `next_token`；与已列 `bump` clone 正交。宜无分配 peek 或 Ident intern。
- [x] **`${…}` 嵌套串跳过按字节 `+2` 吃转义**：`skip_quoted_literal` 按 UTF-8 scalar 跳过 escape/内容（`"`/`'` 共用）；加 `\中` 嵌套测。主字面量路径仍自建 lit。
- [x] **pretty `escape_str` 缺 `\r`，与 lexer 表漂移**：`escape_str` 与 char 模式 fmt 已补 `\r`；加 roundtrip 测。仍欠与 lexer 共用单一 escape 真源。

#### HIR ↔ ty / 控制流

- [x] **双份 free_vars，`Assign` 语义分裂**：`hir/collect_free_vars` 现将 Assign LHS 记为 use；`ty/free_var_names` 直接委托 `all_free_vars`（spawn 捕获与 list_hof 并行检查同源；并行侧更保守）。
- [x] **`break`/`continue` 定型无循环嵌套校验**：`AltReturnState::loop_depth`；`infer_loop` 增减；lambda/`infer_fun` 清零；拒测覆盖裸 `break`/`continue` 与跨闭包。
- [x] **ty demote 公开啃 HIR desugar + `LowerCtx::empty()`**：顺序 `desugar_list_{map,fold}_sequential` 不再吃 `LowerCtx`；`for_each_elem`/`list_accum` 等循环骨架去 ctx；已删 `LowerCtx::empty`。

#### codegen / abi

- [ ] **codegen `roots::type_may_heap` 成第五套「是否堆」**：与已列 value_ty / float_abi / mono / `lambda_lift/heap` 并行；`slots` 复用之。宜并入单一 heap lattice。
- [x] **未知 mut slot `unwrap_or(true)` 当堆根**：`slot_may_heap` 未知→非堆；Assign 缺 `local_tys` 写 `Type::Int`；COW `None` 同步。
- [x] **`emit_memo` 魔数 `4`，不读 `MEMO_TF_MAX_ARGS`**：已改用 `lumia_abi::MEMO_TF_MAX_ARGS`；`const _: () = assert!(… == 4)` 钉死与 C ABI 四槽一致。
- [x] **ADT float/bool field mask 静默 `.min(64)` 截断**：`emit_eq` / AllocAdt / with-update 超 64 字段改为 emit 期 `bail!` ICE；加 mask 单测。
- [x] **musttail 无返回值时静默 `i64 0`**：`emit_musttail_call` 对 void LLVM 结果 `context` ICE（Lumia ABI 恒 i64）。
- [x] **`emit_stack_*` 头布局近拷贝 + 模块注释仍写 Map**：`emit_stack_header` 共用三字头；模块注释改为 List/ADT（Map/Set 不走栈）。
- [x] **`emit_rt_*` 再挖 String/List 符号特判**：`string_receiver_rt_override` 单表覆盖 reverse/take/slice/concat；emit_rt_* 共用。
- [x] **`emit_value_if` 每臂克隆整表 `rooted_slots`**：入口仍 snapshot 一次（musttail 会清空 map）；两臂之间与 merge 前 `restore_root_checkpoint`/`clone_from`，避免 then musttail 污染 else 的编译期根状态。真栈式 length 回滚仍欠。
- [x] **`lumia_abi` 成「tid + opt 阈值 + 域核符号」杂仓**：拆为 `type_id` / `memo` / `opt_caps` / `dense_f64` / `scheduler`；`lib` 再导出。

#### LSP / pkg / 工具链假绿

- [x] **`severity_and_code` 靠 `type:` 前缀，生产 type 诊断常无 code**：`DiagnosticKind` + overlay/partial 显式 kind；LSP `code` 不再啃消息前缀。
- [x] **LSP format 解析失败返回空编辑（假绿）**：parse 失败改为 `Err` → JSON-RPC `-32603`；加拒测。
- [x] **分析成功只清空当前 URI，跨文件诊断可陈旧**：成功时清 load 图全部文件 URI；`last_diag_uris` 合并清上次 import；close 顺带清。
- [x] **`pkg` lock 缺版本写死 `"0.0.0"`**：无 version / 无 dep `Lumia.toml` 时 `bail`，不再假绿。
- [x] **`codegen` feature 半切：slim 仍硬链 `lumia_core`；Build clap 永在**：`lumia_core` 仅 `codegen`；无 feature 时隐藏 `Build` 子命令。
- [x] **VS Code README 设置/vsix 号漂移 + 对账脚本不管配置键**：README→`0.3.9` + `autoParallel`；`check_editor_assets` 对账 version/vsix/settings 键。
- [x] **`scripts/e2e.sh` 游离：名义 e2e 但不进 CI/`check.sh`**：标明非正式冒烟；BUILD/`check.sh` 写明门禁是 `cargo test -p lumia --tests`。
- [x] **位置/着色测例全 ASCII + `lumia_core` crate 文档仍写「SSA-ish」**：`diag` 多字节字节列金样；core 文档改为「树形 ANF / 伪 SSA」。

### 续（2026-08-16 第九轮；不重复上方条目）

对照 `FunKind`/`mono_of` 半收口、选项对象、Option tag 旁路、syntax stamp、codegen 帧状态与 memo 词汇再挖；下列为仍欠项。

#### IR 身份 / 选项 / 黑板旁路

- [x] **`FunKind` / `mono_of` 半迁移，字符串协议仍权威**：`is_lifted_lambda`/`is_val_getter`/`base_name` 改走 `kind`/`mono_of`；表路径经 `with_lifted_lambda_names`（FunKind TLS）+ `fun_ty_from_tables_tls`；mono 回退用共享 `strip_mono_suffix`（仅无 index 时）。
- [x] **编译选项仍四散（DenseF64 Debug 项勾掉后遗留）**：`OptOptions::for_build` 已让 `dense_f64_sr`/`memo_tf` 跟 `release`；CLI 仍可单独覆盖。单一 `CompileOptions` 仍欠。
- [x] **`CoreModule.option_{some,none}_tag` 又一条 Option 旁路**：`lumia_hir::langitem::{OPTION,RESULT}` 定名/变体/默认 tag；lower 注入与 `option_ctor_tags` 消费；ty/mono/codegen 主路径已改走 `is_option*` 助手。
- [x] **`ForeignAbi::from_symbol("lumia_")` 仍靠前缀猜 ABI**：删 `from_symbol`；`foreign "C"`→`C`，dense_f64 合成 stub→`Runtime`。

#### 前端 / 管线阶段

- [x] **`assert_annotate` 在 typecheck 后改写 HIR**：删 HIR 突变；Core lower 按 `assert_files` 为裸 `assert(cond)` 注入 `path:line` 文案（typed HIR 仍 1 参）。
- [x] **syntax 无 `visit`，`stamp.rs` 手写整树 span walker**：`syntax::visit::{map_module_spans,map_expr_spans,…}`；stamp 只做 stamp/offset。

#### 中端不动点 / codegen 状态 / RT 词汇

- [x] **不动点上界汤锅无政策表**：`lumia_abi::fixpoint`（`FLOAT_MONO`/`MONO_CLONE`/`CHANGE_FLAG`/`CLOSURE_CAP_TY`）；触顶仍为静默停。
- [x] **`FrameState` 塞进 nsw_iv 分析缓存**：`NswFacts` + `analyze_nsw` 一次填充；`FrameState::install_nsw`；`slot_i64_const` 仍为 emit 期状态。
- [x] **`funref_locals` 双份传播**：`funref::note_funref_local`（emit `Ignore` AllocClosure / cap-ty `Track`）；全量 FunRef `AnalysisFacts` 仍欠。
- [x] **Memo C ABI `lumia_memo_l2_*` vs Rust `MEMO_TF_*` 词汇分裂**：`lumia_abi` / rt / codegen 钉死「L2=历史冻结符号、Rust 用 `MEMO_TF_*`/`T_f`」；已有 memo_tf IR 对账测。
- [x] **`LitMap`/`LitSet` 与物理布局同枚举**：标注 PE hint + `is_pe_hint`；ReprSelect 显式降为 SmallMap/HeapSet；加测。
#### 文档新鲜度

- [x] **DESIGN/BUILD「最后更新」远落后于代码**：戳 2026-08-16；注明以 Todo/代码为准。

## 已落地记录

历史收口与功能完成项已从本文件移除；详见 git 历史与 [docs/BUILD.md](docs/BUILD.md)。


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
- [ ] **`is_heap_payload` = 进程堆 Mutex + `heap_set` 查找**：`common.rs` `heap_gen`/`is_heap_payload`。COW 已对 List/ADT 信任 tid；**`eq` / `show` / `println_auto` / `hash` / `ord`（双标量）**已用 `may_be_heap_payload_bits` 跳过 Int/Bool/FunRef 探堆。**写屏障 / Map 非 Float 键 / 未过滤热点**仍每点探一次。最大单点税。
- [ ] **GC `mark_value` 每边再抢锁**：大对象 mark 对每个子字仍 `with_heap`/`is_heap_payload`。宜整波持锁 + 信任已消毒 mask。
- [ ] **shadow-stack `root_push/pop` 每临时都抢堆锁**：`mutator.rs`；热路径大量 List/ADT Let 付 Mutex。宜 TLS 根栈 + 仅 full-mark 时 shade。
- [ ] **Memo lookup 整段 `with_heap`**：`memo.rs` 热命中与堆争用。宜世代/epoch，仅 store×full-mark 持锁。
- [ ] **分配路径多次加锁**：inhibit 检查 → maybe_collect → `finish_alloc` 再插 young/`heap_set`。宜单次 `with_heap` 覆盖；nursery bump 延迟入 set。

### 分配与慢路径
- [ ] **`lumia_show` / 嵌套 show 多段 `String` 分配**：容器插值/`println_auto` 锁+分配密集。宜单缓冲写入。
- [x] **`map_get` 总堆分配 Option ADT**：miss 路径改为按 `none_tag` 永生单例（`RC_SHARED` + `Heap.option_none`）；hit 仍堆分配 `Some`。

### 并行与调度
- [ ] **spawn 仍克隆 scope**：默认纤程栈已降至 64KiB（`LUMIA_FIBER_STACK_KB` 可覆盖）；细粒度 spawn 仍克隆 scope，宜栈 freelist / 更轻 scope。

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

- [ ] **Core 堆/Float ABI 定型上帝模块**：`lambda_lift/float_abi.rs` ≈3583 行（生产+同文件测试；持续膨胀）；`local_heap_ty` 单函数超长穷举。同层并行 `value_ty::join_value_tys` / `float_abi::join_heap_tys` / `mono/ret_ty::join_fixed_ty` 三套合流近拷贝（Float 优先臂注释互相引用）；**codegen 再有第四套** `closure_cap_tys::prefer_cap_ty`（闭包捕获定型，逻辑同族）。`List(Int)` 作「可能堆」软占位再靠 `prefer_concrete_*` 让位。是反复打 Float/channel ABI 补丁的结构根因——应收成单一 lattice / 表驱动 walker，占位用显式未知类型而非 `List[Int]`。
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
- [ ] **`lumia_rt` / `lumia_syntax` 公共 API 过宽**：rt `lib.rs` 大量 `pub use` 展开内部模块；syntax `pub use ast::*`。对比 hir/ty/codegen 的 `pub(crate)` 更收敛——重构边界模糊。
### 续（2026-08-15 第二轮；不重复上方条目）
- [ ] **Value→Type 三套完整并行 walker**：除已列的 join/prefer 近拷贝外，`value_ty`（≈955，含 `infer_value_ty_ctx` + 拆出的 `builtin_value_ty`）、`float_abi::{local,block}_*_heap_ty`（`local_heap_ty` 单函数 ≈687）、`mono/ret_ty`（≈720）各自重匹配几乎全部 `Value`/`Builtin` 臂；ABI 补丁常需改三处。应收成单一 typed analysis API，heap/mono/codegen 作薄客户端。
- [ ] **`ClosureCap.as_float` + `float_cap_fixup` 半吊子通道**：IR 上可变 `as_float` 旗标（`rewrite` 写入 → `float_cap_fixup` ≈1231 行事后补丁 → codegen `emit_calls` 消费），与 `param_tys`/`ret_ty`/闭包捕获表并行。Float 捕获 ABI 应只从 typed cap 表导出，删掉事后 mutation。体量/职责继续膨胀见第五轮。
- [ ] **`mono/specialize.rs` 上帝模块**：≈2135 行集 clone 发现、改写、ret refresh、forwarder 消除、FunRef HOF、Option/Result 载荷规则于一身；几乎每个 mono ABI 修复都落这里。宜按 collect / rewrite / ret_refresh / forwarders 拆分，并与 `ret_ty` 共享 lattice。
- [ ] **codegen `nsw_iv` 第二块基准形岛屿**：`nsw_iv.rs` ≈1071 行（Collatz/`3*x`、fib、matmul 形 peep），经 `emit_fun` 焊进每个函数 emit。与已列 `*_sr` 同病但未收录——通用 NSW 被热核形状绑架。宜迁 opt / feature-gate，codegen 只发 NSW 标记。
- [ ] **Core IR 嵌 `lumia_syntax::{BinOp,UnOp}`**：`ir.rs` 算术节点直接用 syntax token 枚举 → opt/codegen 中后端继续依赖 `lumia_syntax`（与已列 HIR `Builtin` 穿透同族、另表面）。lower 边界应收成 `CoreBinOp`/`CoreUnOp`（或 opcode id）。
- [ ] **Prelude `Option`/`Result` 靠字符串魔改**：`mono/key.rs`、`ret_ty`/`specialize`、`ty/alt`、`ty/infer/expr`、`hir/lower/items` 等处硬编码 `"Option"`/`"Result"` 特判（载荷/擦除/mono）。stdlib ADT 成编译器魔法，非 langitem。宜 prelude 注册表（tag、载荷元数、mono 规则）供 ty/core 消费。
- [ ] **SSA `Local` + 字符串 `Name`/`Assign` 双寻址**：`Value::Name(String)` + `Op::Assign { name }` 与 SSA 并存；ABI/`slot_tys` 必须双轨跟踪。槽位应统一 `Local`/`SlotId`，名字仅调试打印。
- [ ] **`InferValueCtx` 可选表蔓延 / `FunIndex` 仅 mono**：`value_ty` 上下文堆 ≈8 个 `Option<&HashMap<…>>`；`fun_index` 仅 mono 用，而 float_abi/fixup/channel_hint/codegen 反复手拼 `fun_ret_tys`。缺共享 `ModuleTables` → 表装配拷贝。`CodegenTypeTables` 已存在但几乎只服务 codegen（半收口见第五轮）。
- [ ] **Builtin→RT 符号在 `BuiltinInfo` 外覆盖**：codegen `builtin/mod.rs` 按 `Type::List` 把 `ListLen`/`MapSet`/`ListGet` 改道 `lumia_list_len`/`lumia_list_set`/`lumia_list_get`（绕开 info 表里的多态符号）。表驱动 emit 被字符串特判挖空；宜把单态分发收进 `BuiltinInfo` 或 Core opcode。
- [ ] **HIR `visit` 未被 `lumia_ty` 使用**：`hir/visit.rs` 已有 `for_each_expr`，但 ty 的 `effects`/`alt`/`parallel`/`product_resolve`/`traits`/`free_vars` 全手写 walker（与已列 Core `visit` 欠债同型、前端侧）。新 `Expr` 臂易漏；ty 应变默认走 hir visit。
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
- [ ] **TLS `BACKEND` 空壳罩进程 `Heap`**：`gc.rs` `thread_local! BACKEND` 调 `MmBackend`，真状态在进程 `Heap` Mutex；方法再入 `with_heap`。看似可插拔/每线程，实为进程全局 + TLS 门面（与「写死 MarkSweep」正交）。宜去掉伪装或真做每线程 nursery。
- [ ] **Task ↔ GC ↔ list-par 硬耦合**：GC shade 拉 `task::snapshot_sched_gc_roots`；fiber/channel 调 alloc/root；`list/par` 看 `task_runtime_active()`。三子系统无法独立演化；锁序是跨模块不变量。宜窄接口（根枚举 / 「禁并行」谓词）+ 文档化锁序。**第七轮**：全仓仅两处行内 `heap → sched` 注释，见续「锁序几乎无文档」。
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
- [ ] **`lower_hir` 编排中端遍，而非纯 HIR→Core**：末尾串 lift→hint→directize→trait→6×(fixup+mono)→stubs。与已列「魔法迭代上界」同管线、但是**所有权**债。宜 lower 纯翻译；具名 Core pass 管道 + 阶段不变量。
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
- [ ] **`show_methods` 仅 Show 旁路表**：在通用 `trait_methods` 外再特判 Show。其它 trait 无对称快路径——又一层魔法。
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
- [ ] **LSP 诊断仍恒 Error，缺 relatedInformation/tags**：消息前缀可填 `code`；severity 仍恒 1；relatedInformation/tags 仍缺。
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
- [ ] **Core lower 残留 `.expect`**：Alt/With 已 `note_ice`+`Err`；`control.rs`/`call.rs` 等仍有 `.expect("ICE: …")`。宜一律 `Err(Ice)`。
- [ ] **锁序缺 CI 禁令**：`lumia_rt` crate 文档已有 Lock order（heap→sched）；memo/dict/mutator/channel 未入表；CI 禁令仍欠。

#### 前端 / RT / 包装 / 文档
- [ ] **`std.linalg` 仍占语言标准库**：`cn`/`efe` 已迁 `extras/`，`linalg.lm` 仍几乎全是 `foreign`→`lumia_f64_*`，且在 `std_mod` 白名单。域模块迁出不完整。宜迁 `extras.linalg`（或等价），RT 域核走 feature。
- [ ] **RT `dispatch.rs` = 开放方法的运行时孪生**：`lumia_len`/`concat`/`set`/`elems`/… 按 `type_id` 分发，与 ty 的 `*_vars` 同族语义分属两处。宜单一能力表生成/对账。
- [ ] **`string_io.rs` 混装 String / IO / stdin / trap**（≈498）：核心字符串表示与 I/O 策略、trap 耦在一起。宜拆 `string` / `io` / `trap`。
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
- [ ] **Parser 用户错误用 `TokenKind` 的 `Debug`**：`expected {kind:?}, found {:?}`（`parser/mod`/`block`/`expr`）。宜 `display()` / 期望集合文案。
- [ ] **恢复路径注入恒等毒桩 lambda**：`parse_val_item_resilient` 失败体换成 `{ _1,_2 -> _1 }` 类 stub 仍进符号表/定型。宜 `Hole`/`Error` 类型，或恢复项不绑定 scheme。
- [ ] **`peek_kinds` 旁路重词法 + Ident 再分配**：临时 `Lexer` 调 `next_token`；与已列 `bump` clone 正交。宜无分配 peek 或 Ident intern。
- [ ] **`${…}` 嵌套串跳过按字节 `+2` 吃转义**：主字面量按 UTF-8 scalar，插值内跳过可落非法边界。宜与 `lex_string` 共用安全扫描。
- [ ] **pretty `escape_str` 缺 `\r`，与 lexer 表漂移**：fmt→parse 含 CR 不保真。宜单一 escape 真源 + 对账测。

#### HIR ↔ ty / 控制流

- [ ] **双份 free_vars，`Assign` 语义分裂**：`ty/free_var_names` 把 LHS 记 free（spawn 捕获）；`hir/collect_free_vars` 只走 RHS（`list_hof` 并行安全）。宜单一 API。
- [ ] **`break`/`continue` 定型无循环嵌套校验**：直接 `(Unit, Pure)`；无 `loop_depth`；无拒测。宜 typing 跟踪循环深度。
- [ ] **ty demote 公开啃 HIR desugar + `LowerCtx::empty()`**：`parallel.rs` 调 `desugar_list_*(&LowerCtx::empty(), …)`。宜纯 desugar 函数或不依赖 LowerCtx 的 typed pass。

#### codegen / abi

- [ ] **codegen `roots::type_may_heap` 成第五套「是否堆」**：与已列 value_ty / float_abi / mono / `lambda_lift/heap` 并行；`slots` 复用之。宜并入单一 heap lattice。
- [ ] **未知 mut slot `unwrap_or(true)` 当堆根**：`emit_fun/slots.rs`；与「未知→Int」正交——是 **未知→堆** 反方向缺省。
- [ ] **`emit_memo` 魔数 `4`，不读 `MEMO_TF_MAX_ARGS`**：`.take(4)` / `[IntValue; 4]`。宜只引用 abi。
- [ ] **ADT float/bool field mask 静默 `.min(64)` 截断**：`emit_eq.rs`；>64 字段丢位不报错。宜编译期拒绝或宽 mask。
- [ ] **musttail 无返回值时静默 `i64 0`**：`tco.rs`。宜 ICE。
- [ ] **`emit_stack_*` 头布局近拷贝 + 模块注释仍写 Map**：`stack.rs`「List / Map / ADT」但 Map/Set 不走栈。宜共用 header helper；改正注释。
- [ ] **`emit_rt_*` 再挖 String/List 符号特判**：`ListReverse`→`lumia_str_reverse` 等。坐实 BuiltinInfo 外覆盖的 emit_rt 近拷贝面。
- [ ] **`emit_value_if` 每臂克隆整表 `rooted_slots`**：嵌套 if/musttail 用深拷救根状态。宜 checkpoint / 长度回滚。
- [ ] **`lumia_abi` 成「tid + opt 阈值 + 域核符号」杂仓**：`SPECIALIZE_CONST_*` / `DENSE_F64_TRAMPOLINE_SYMS` / `SCHEDULER_*` 与 `TYPE_*` 同文件；`Cargo.toml` description 仍写「type_ids, memo caps」。宜拆模块或改 description。

#### LSP / pkg / 工具链假绿

- [ ] **`severity_and_code` 靠 `type:` 前缀，生产 type 诊断常无 code**：多文件/分析路径发裸 `message()`；单测只喂已带前缀串。宜结构化 `DiagnosticKind`。
- [ ] **LSP format 解析失败返回空编辑（假绿）**：`formatting.rs` `Err → vec![]`。宜返回 error / 不响应成功空集。
- [ ] **分析成功只清空当前 URI，跨文件诊断可陈旧**：`Ok` 分支 `vec![(uri, [])]`；先前发到 import URI 的诊断不清理。
- [ ] **`pkg` lock 缺版本写死 `"0.0.0"`**：锁「绿」但无信息。
- [ ] **`codegen` feature 半切：slim 仍硬链 `lumia_core`；Build clap 永在**：无 feature 时 Build 体内才 `bail`；LSP 仍拖 Core。宜 core 仅 `codegen` 依赖；隐藏/拆 Build 子命令。
- [ ] **VS Code README 设置/vsix 号漂移 + 对账脚本不管配置键**：README 仍 `0.3.5.vsix`、Settings 漏 `autoParallel`；`check_editor_assets` 不对账 settings/README/版本。
- [ ] **`scripts/e2e.sh` 游离：名义 e2e 但不进 CI/`check.sh`**：实际门禁走更宽的 `cargo test -p lumia --tests`。宜删、改名，或明确「非正式入口」。
- [ ] **位置/着色测例全 ASCII + `lumia_core` crate 文档仍写「SSA-ish」**：BUILD/DESIGN 已改「树形 ANF / 伪 SSA」；`core/lib.rs` 残留。宜多字节位置金样；对齐 crate 文档。

## 已落地记录

历史收口与功能完成项已从本文件移除；详见 git 历史与 [docs/BUILD.md](docs/BUILD.md)。


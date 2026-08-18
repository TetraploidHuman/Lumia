# Lumia — 本轮未完成 / 待推进

记录经查实、尚未完整修复或需更大设计落地的问题。语义以 [docs/DESIGN.md](docs/DESIGN.md) 为准；分期见 [docs/BUILD.md](docs/BUILD.md)。
已确认落地的历史项见下方 `[x]` 与 git 历史。

## 性能优化清单（2026-08-17）

面向 **编译更快 / 跑得更快 / 更省内存**。与下方「性能债」「架构卫生」交叉处只写动作与优先级，细节仍以原条目为准。

### 编译速度（主机侧：解析 → 中端 → LLVM → 链）

- [ ] **Token / Ident 实习 + 少 clone**（高）：[`parser/mod.rs`](crates/lumia_syntax/src/parser/mod.rs) `bump` 已改 `mem::replace`；`expect_ident` 已 move；**Checkpoint 已改为按 span 重 lex（不再 clone Token）**。仍欠 `StringInterner` + span-only token。
- [ ] **前端 arena（syntax/HIR）少拷 Expr 子树**（高）：脱糖与 `list_hof` 大量 `body.clone()`；大模块峰值 RSS 与解析 CPU 同涨。
- [ ] **import 增量编译单元**（高）：整模块内联重解析/重类型/重 opt/重 codegen。无 ABI 边界则大工程与 LSP 编译墙钟无解。← 与「import 整模块内联」同债。
- [x] **Memo plan 单次扫模块**（高）：[`memo/plan.rs`](crates/lumia_opt/src/memo/plan.rs) 一次 `max_const_arg_reuse_by_fun` 收集 `fun → const-arg 最大频次`；`slots_cost_ok` 只查表（原每候选全模块扫 ≈O(n²)）。
- [x] **Escape 固定点降本**（中高）：worklist + Call 图 reverse edges；预算耗尽只强制**仍开放 SCC**（非整模块）全参逃逸（2026-08-18）。**`Call`/`FunRef`/`AllocClosure` 均 `CallTarget`+`FunId`**；Escape 入口 `resolve_module_call_fun_ids`（2026-08-18）。
- [x] **中端少 clone `CoreFun` 体**（中高）：inline / specialize_const / **mono（含 FunRef directize）均已 `Arc` 体模板**（2026-08-18）。Inline 槽名改为 **`$s{id}`（同 Local 计数器）** 并保留脱糖前缀；仍欠真 `Assign`/`Name`→Local（2026-08-18）。
- [ ] **树形 IR → CFG 或统一 visitor**（中）：每 pass 自写嵌套 walker；CFG/`visit` 默认入口降中端编译时间与漏扫。← 与「树形 Core」「visit 未成默认入口」同债。
- [x] **Release LLVM 二次 `verify` 可闸**（中）：[`codegen/lib.rs`](crates/lumia_codegen/src/lib.rs) emit 后仍 verify；O3 后再验需 `LUMIA_VERIFY`（见 BUILD §8）。
- [x] **workspace `[profile.release]`**（中）：根 `Cargo.toml` thin LTO + `strip = "debuginfo"`；`lumia_rt` `codegen-units = 1`。
- [ ] **默认/文档推 `llvm-dynamic` 链编译器**（中）：BUILD §4.2 已写开发用法；仍可考虑把开发默认切到 dynamic。
- [ ] **Debug/check 可选用 LLVM `-O1`/`fast`**（低中）：`!release` 现为 `OptimizationLevel::None`；需要可跑产物时不必全 O3 中端+LLVM。
- [ ] **字符串键 → FunId / 实习名**（中）：`Call` / `FunRef` / `AllocClosure` 已统一 [`CallTarget`](crates/lumia_core/src/ir.rs)（`name`+可选 `FunId`）；Escape resolve 三者一并填 id（2026-08-18）。仍欠 mono key / Ident 实习、少 clone 名。
- [ ] **`Type` 树实习 / 封闭 `CoreTy`**（中）：推断克隆开放 `lumia_ty::Type`；中端宜封闭 ABI lattice，前端可 arena+intern ADT 名。← 与「中端仍吃开放 Type」「CoreTy」同债。
- [ ] **LSP 脏模块缓存**（中）：进程 `Mutex` + Full re-analyze；format 再严格 parse。增量分析降 IDE 编译感延迟。

### 运行速度（生成码 + RT）

- [x] **Iota 虚拟列表落地**（高）：RT `TYPE_LIST_IOTA`（`lumia_range`）O(1) 建表；get/len/take/slice/par_map 走虚路径；`set` 叠 patch；**相邻 Iota concat 仍虚**；unique reverse/sort/sortBy/concat 吃 spare。Core `ListRepr::Fused` 仍为 HIR 脱糖保留标签（ReprSelect 不发出）；逃逸管道按普通 `map`/`filter` 物化（2026-08-18）。
- [ ] **HOF 融合超出 fold 汇合**（高）：build/`flatMap`/`any`/`all`/`find`/`len`/`isEmpty`/`contains`/`toSet`/`toMap`/`toList`；**`for-in` 扫 map/filter/take/drop/flatMap**；Let-bound for-in / contains / get/len/take/drop；take|drop×消费端；**`toMap().get/contains` 单键扫配对流（last-wins Option；≥2 次仍建 Hash）**；**`toSet().contains` 短路扫**；Loop 内 get 不脱糖（2026-08-18）。
- [x] **扩大 NSW/`nuw` Int 算术**（高）：默认 `llvm.*.with.overflow`；`nsw_iv` 已覆盖 IV/树/字面量 + const-upper 非负 IV `+`/`*` + **开放排他 `i < n` 最坏 `U=MAX-1`（仅 `i+1`）** + **named-upper 的开放 inclusive `i <= n`（嵌套在已有 `iv_upper` 下）** + **safe Div / 小 const Rem 不依赖 IV 界** + **有界非负 IV 对加/乘 `i+j` / `i*j`**（2026-08-18）。仍欠更广一般算术。
- [ ] **更富内联**（高）：`INLINE_MAX_OPS=64`（Domain SR 已覆盖 `$c_` 克隆后恢复；2026-08-18）；`IndirectCall`+`FunRef` 已可内联。仍欠热度、捕获闭包栈分配 / defunctionalize；仅 Release。
- [ ] **GC 根跨块 last-use**（高）：同块无 safepoint 跳过 `root_push` 已落地；**纯 `If` 臂 / 夹心纯 `If` / GC-free `Loop`（含体内用途）** 已纳入 last-use 消根；**有 safepoint 时 LIFO last-use 早 `root_pop`**（死后且在栈顶的 SSA/参根立刻弹出；`Lambda` 仍保守，2026-08-18）。仍欠非栈顶死根 / 通用跨块精细消根。
- [ ] **未逃逸闭包 / 小 ADT 栈化**（高）：escape 已有，物化仍偏堆；DESIGN 栈/SROA 路径未吃满。
- [ ] **通用 `List[Float]` 向量化**（中高）：`dense_f64_sr` 形状匹配已扩 `var out = xs` / `val out = xs` 别名（scale/clamp/fill 测例已覆盖）；未匹配仍标量 SSA + RT list。
- [x] **Set 走 Overlay 类持久更新**（中高）：Map 同款 `[-1][parent][dn][e…]`；Hash `insert` 叠 delta（≤`SMALL_CONTAINER_MAX`）再 materialize；mark/evacuate/show/eq 已接（2026-08-18）。
- [x] **Map/Set 唯一 RC 原地更新**（中高）：Map/Set alloc `rc=1` + COW；unique overlay 更新/追加（壳按 `OVERLAY_MAX` 预分配）；unique Hash 在负载允许时原地 upsert；**线性表预留容量后 unique 追加**；Set 已存在元素 identity；**remove 未命中 identity**（含 overlay）；**unique 线性 compact**；**unique Hash tombstone 删除**（`n > SMALL_MAX`）与 **原地 demote 线性**（`n ≤ SMALL_MAX`）；**overlay 仅 delta 键 remove 不 materialize**（父键命中仍 flatten）；**codegen `s = s.insert/remove` 与 `xs = xs.reverse/sort/sortBy` 走 COW consume**（2026-08-18）。
- [ ] **Map/Set `Small*` / `BuildFused` / 单键短路**（中）：HIR 已对未逃逸 **单次** `pipe.toMap().get/contains`、`pipe.toSet().contains` 做线性扫（2026-08-18）；仍欠运行时 `SmallMap` / 逃逸后补建哈希。
- [ ] **进程共享 Memo `T_f`**（中）：现 TLS-only；OS worker 间不共享命中。
- [ ] **领域 SR 迁出 codegen → opt**（中）：batch1+2 whole-fn 已迁；剩 floatOrbit IR + steps/trial-div。**0 参 `$c_` 克隆**可命中；**Release 管道在首次 `SpecializeConst` 前增加 Domain SR**；**`memTrafficChecksum`→`lumia_mem_traffic_checksum`（i64 SIMD）**；**floatOrbit x4 为 `<4 x double>`**（2026-08-18）。
- [ ] **List-par 与 GC 再解耦**（中）：调度/根/nursery 边界继续收窄，降并行 HOF 停顿。
- [ ] **TCO 覆盖面审计**（低中）：`musttail` 已有；查漏更多自递归形状。

### 内存占用（编译器峰值 + 程序运行）

- [ ] **解析/HIR arena + 少 Expr clone**（高）：同上前端 arena；直接压编译器峰值。
- [ ] **少根槽 / 更短 live root**（高）：跨块消根 + **LIFO last-use 早 pop**（2026-08-18）后 nursery mark 扫描更短、停顿更小。非栈顶死根仍占槽。
- [x] **inline/mono 避免整函数体 clone**（中高）：inline + specialize_const + mono FunRef 路径均 `Arc` 模板（2026-08-18）。
- [ ] **统一持久容器层**（中高）：List/ADT RC-COW vs Map/Set Overlay；**delta 壳**（nbytes/parent/dn/clamp/mark-parent）已抽 [`container_delta.rs`](crates/lumia_rt/src/container_delta.rs)，Map/Set Overlay + List patch + GC 已接（2026-08-18）。仍欠共享 materialize / 全量合流。
- [ ] **ReprSelect / Lit\* 推更多栈与永生小值**（中）：空 List/Map 永生已有；小未逃逸 ADT/Map/Set 栈路径仍欠。
- [ ] **Nursery / TLS LAB 按负载可调**（中）：已有分代+LAB；LAB 尺寸与 young limit 可工作负载调参或自适应。
- [x] **DCE 收紧 `builtin_may_trap_or_effect`**（低中）：`Range`/`RangeInclusive`/`AdtTag` 已移出陷阱集；DCE 已迭代到不动点以清掉死 Builtin 的残留参数（2026-08-17）。
- [x] **链时 `--gc-sections` + RT thin LTO**（低中）：link 已 gc-sections；workspace release thin LTO + `lumia_rt` CGU=1 已落地（2026-08-17）。
- [ ] **`--mm=arc` 仍非近优**（低）：分代 GC 已主路径；ARC 仅为延迟敏感愿景。

### 建议推进顺序（ROI）

1. **跑得更快**：HOF 续（`.get`/Let 脱糖 + **toMap 单键扫**已落地；Iota 仍 RT）→ 广 NSW → 少 GC 根（纯 If / GC-free Loop 已扩；**LIFO last-use 早 pop**）→ 闭包/小值栈化；Set overlay 已落地。
2. **编译更快**：Ident/token 实习 → escape FunId on IR（**CallTarget 已落地**）→ 少 CoreFun clone（inline/specialize_const/mono Arc 已落地）→（中期）增量模块。
3. **更省内存**：arena + 消根 + 持久容器统一；`[profile.release]` thin LTO 已落地，续推 arena/消根。

### 基准快照（2026-08-18 Collatz/mem 单项，相对改前）

单项（冷却后样本；热节流下会虚高）：

| 核 | 改前 | 改后 | 备注 |
|----|------|------|------|
| `collatzTotal` | ~0.042s | **~0.033–0.035s** | sequential 跳过 Syracuse 命中后的 `2v` 扇出；双跳 Syracuse；unchecked |
| `collatzStrided` | ~0.039s | **~0.034–0.035s** | 双跳 + unchecked；仍保留 doubles 扇出 |
| `memTraffic` gather | ~0.059s | ~持平 | 试过 8 路/双流/排序再 gather，均更慢；保持 4 路+prefetch |

### 基准快照（2026-08-18 SIMD 续，cpu/memo，相对上一轮 0.16s）

相对 Collatz 恢复后全量（cpu **0.16s** / memo 8.4×）：

| 套件 | 上次 | 本轮 | 相对上次 |
|------|------|------|----------|
| `bench_cpu` | 0.16s | **~0.15–0.16s** | 持平略好（`floatOrbit` x8：单核 0.0075→**0.005s**） |
| `bench_memo` | 8.4× | **8.7×** | 噪声 |

试过但回退/未采用：`vpgatherqq` LCG gather、Mandelbrot AVX2 存回标量控制（均变慢）。保留：`floatOrbit` `<8 x double>`（`n%8==0`）、memTraffic gather **4 路标量展开**+prefetch。

注：长编译/CN naive 后 CPU 降频会把 Collatz 测成 ~0.12s、整套 ~0.32s——属机器热节流，不是算法回退。

### 基准快照（2026-08-18 Collatz 回归排查后 `bench_all`，相对上一轮 0.32s）

全绿。上一轮全量 cpu **0.32s** 主因是 Collatz RT 墙钟异常（单核 ~0.10s；同算法 `rustc -O` / 干净 Release RT ~0.04s）。非算法回退：clang 链的 `liblumia_rt.a` 曾处于慢态；`cargo clean -p lumia_rt` **不会**清 Release 产物，需 `--release`。兼有热节流干扰。

| 套件 | 上次（异常轮） | 本轮 | 相对上次 |
|------|----------------|------|----------|
| `bench_cpu` | 0.32s | **0.16s** | **恢复 ~2×**（collatzT ~0.043s） |
| `bench_memo` | 10.9× | **8.4×** | 噪声 |
| `bench_cn_hot` | 126×（k 0.24s） | **125×**（k **0.25s**） | 持平 |
| `bench_cn_step` | 130×（k 0.30s） | **131×**（k **0.29s**） | 持平 |
| `bench_cn_efe` | 42×（k 0.063s） | **41×**（k **0.066s**） | 持平 |
| `cn_fuse` / `forward` / `strict` | 1.6 / 1.8 / 1.8 | **1.6 / 1.8 / 1.9** | 持平 |
| `bench_task` | OK | OK | 持平 |

防护：`profile.release.package.lumia_rt` 显式 `opt-level=3`；`bench_all` 先 `cargo build -p lumia_rt --release`；collatz 测例加 Release 墙钟上限。

### 基准快照（2026-08-18 SIMD 后 `bench_all`，RUNS=3，BENCH_SHIELD=0）

全绿。相对上一轮正式全量（深夜：cpu 0.16s / memo 8.3× / cn_* 如下）：

| 套件 | 上次 | 本轮 | 相对上次 |
|------|------|------|----------|
| `bench_cpu` | 0.16s | **0.32s** | **变差 ~2×**（核分解：`collatzTotal/Strided` ~0.04→~0.10s；其余接近；checksum 仍绿） |
| `bench_memo` | 8.3× | **10.9×** | 倍率噪声 |
| `bench_cn_hot` | 120×（k~0.25s） | **126×**（k **0.24s**） | 持平 |
| `bench_cn_step` | 163×（k~0.29s） | **130×**（k **0.30s**） | kernel 持平；倍率随 naive |
| `bench_cn_efe` | 44×（k 0.061s） | **42×**（k **0.063s**） | 持平 |
| `cn_fuse` / `forward` / `strict` | 1.6 / 1.9 / 1.8（k 0.092 / 0.269 / 0.297） | **1.6 / 1.8 / 1.8**（k **0.091 / 0.266 / 0.294**） | 持平 |
| `bench_task` | OK | OK | 持平 |

SIMD 轮已落地：`floatOrbit` `<4 x double>`、`memTraffic`→RT+i64 SIMD。`cpu` 变慢主因是 Collatz RT 墙钟，不像 Domain SR 漏改写（`$c_` 仍为 2–4 ops 薄包装）。

### 基准快照（2026-08-18 深夜，`bench_all` RUNS=3，BENCH_SHIELD=0）

全绿。摘要（median）：

| 套件 | 结果 |
|------|------|
| `bench_cpu` | **0.16s** |
| `bench_memo` | **8.3×** |
| `bench_cn_hot` | **120×** |
| `bench_cn_step` | **163×**（naive 噪声大；kernel ~0.29s） |
| `bench_cn_efe` | **44×** |
| `bench_cn_fuse` / `forward` / `strict` | **1.6× / 1.9× / 1.8×** |
| `bench_task` | OK |

本轮：Release 管道 **Domain SR 提前到首次 SpecializeConst 之前**，`$c_` 克隆从厚循环体变为 2–4 ops 薄包装。

## 语义与运行时

- [ ] **Task/Channel 更大设计债**：ready_home/handoff/sweep、`home_coro`/`scan_ptrs`、`SchedCore: Send` 已落地（见下方 `[x]`）。仍欠 **非 RT `Drop`（C-unwind TaskFn ABI）**；堆 Mutex 下增量 mark 非真并行。

## 性能债（2026-08-15 审计确认）

### 运行时热路径锁与探堆
- [x] **`is_heap_payload` nursery 无锁探**：published range + cursor + FREE；`tid==0`/FWD 回落 Mutex。系统/old 仍 `heap_set`。无 pointer tagging。
- [x] **nursery evacuating minor + TLS LAB**：bump survivors 拷贝进 old + rewind；去双写；**仅 process heap `publish_range`**；mutator TLS LAB（`claim_lab` + STW 前 flush）。

### 并行与调度
- [x] **纤程栈 freelist**：TLS `take_fiber_stack` / `recycle_fiber_stack` / `recycle_coroutine_stack`。

### 中端优化缺口（相对 DESIGN §7.2）
- [ ] **融合：消费端已扩**：build/`flatMap`/`any`/`all`/`find`/`len`/`isEmpty`/`contains`/`toSet`/`toMap`/`toList`/`.get`/`.take`/`.drop`；**`for-in` 扫 map/filter/take/drop/flatMap**；Let-bound for-in 与 get/len/take/drop/contains；**`toMap().get/contains` / `toSet().contains` 单次查找不建 Hash**；Loop 内 get 不脱糖（2026-08-18）。Core `ListRepr::Fused` 保留；Iota 仍 RT `TYPE_LIST_IOTA`。
- [ ] **Inline 仅体积阈值且仅 Release**：阈值已提到 64 ops（`$c_` Domain SR 修好后）；`IndirectCall`+`FunRef` 已内联。仍欠热度；捕获闭包恒堆分配（escape → `AllocClosure`）。
- [ ] **默认 Int `+/-/*` 走 `llvm.*.with.overflow`**：`nsw_iv`（opt）已覆盖 rem/unit/tree-acc、ge1 safe-div、Collatz、非负−非负 Sub、字面量 +/\*、const-upper IV、**开放排他 `i<n`→`i+1`**、**named-upper 开放 `i<=n`**、**无界 safe Div/Rem**（2026-08-18）；codegen 对非负 NSW `Add`/`Mul` 兼标 **`nuw`**。仍欠更广一般算术。
- [ ] **堆类型 Let 默认 `root_push`**：ephemeral + 同块无 safepoint 的 `let_skip_root_no_safepoint` 已落地；**纯 `If` / GC-free `Loop` 嵌套用途与夹心** 已消根；**有 safepoint 时 LIFO last-use 早 `root_pop`**（2026-08-18）。仍欠非栈顶死根；`Lambda` 与 AdtField→call 仍保守 retain。
- [ ] **通用 `List[Float]` 向量化靠 `dense_f64_sr`**：未匹配形状仍标量 SSA + RT list；非通用向量管线。
- [x] **Memo plan 单次扫模块**：`max_const_arg_reuse_by_fun` 一次收集 reuse（2026-08-17）。
- [x] **Escape 固定点仍贵**：已改 worklist + 开放 SCC 强制（2026-08-18）；**Call 已挂 `FunId`（`CallTarget`）**（2026-08-18）。

## 工具链

- [ ] **`--mm=arc` / 可插拔 GC 仍非优先**：分代 STW minor + 增量并发 full mark 已落地；ARC 后端仍为愿景。

## 架构卫生

### 结构 / 一致性（仍欠）
- [ ] **Core Float ABI / `local_heap_ty` 仍厚**：`float_abi/` 已按相位拆分，`prefer`/`join` 已迁 `value_ty/join`。仍欠与 `value_ty` / `mono/ret_ty` 整 walker 合流。
- [ ] **领域 SR 仍侵入 codegen + RT**：batch1+2 whole-fn 已迁 `lumia_opt/domain-sr`；codegen 仍留 steps/trial-div/floatOrbit IR；仍欠 RT 域核 Cargo feature。
- [x] **`dense_f64_sr` opt 侧门闩**：Cargo feature `dense-f64-sr`（默认开）；`lumia` `codegen` 开启、`codegen-slim` 关闭。仍欠 codegen 领域 SR 分层。
- [ ] **Core IR 穿透携带 `lumia_hir::Builtin`**：中后端必须依赖 `lumia_hir`+`lumia_syntax`；宜 Core 自有 opcode/元数据。
- [ ] **自动并行决策跨 HIR→ty 两阶段**：`list_hof` 先升 `ListPar*`，`finalize_auto_parallel` 再 demote；策略散在前端两层。
- [ ] **跨层错误类型分裂**：`LocatedError` / `TypeError` / `Result<_, String>` / `anyhow`；诊断易丢 span。
- [ ] **库路径 panic vs Result 不一**：`lumia_ty` alt/PRELUDE_CTOR 已改 `TypeError`/`try_new`；`lumia_core` lower 等对「理论不可达」仍可有 `expect`（非 test）。
- [ ] **双前端管线分叉**：`compile_source_to_core*`（单测）vs CLI/`check_program`（loader+std）；单测易漏 import/包路径。
- [ ] **`visit.rs` 未成默认入口**：共享 `for_each_*`/`collect_*` 已扩；memo CSE/fold、**`specialize_const`**、memo plan 的 Loop 臂已走 visit。生产 `*_sr` 匹配与 memo plan 的 If 分叉合并仍手写。
- [ ] **Windows 工作流仍薄**：`env.ps1` 已对称 PATH/LIB。仍欠 Nix 级发现与完整 `.ps1` 工作流。

### 续（2026-08-15 第二轮）
- [ ] **Value→Type 三套 walker 未合流**：`builtin_value_ty` / `join_fixed_ty` / gated via / float 薄包装已大幅共用。仍欠单一 walker（float_abi Float soft vs `ret_ty`；开放 Map / MatchFail bottom 等残留分叉）。
- [x] **`ClosureCap.as_float` 已删除**：旁路 `float_cap_idxs` + [`abi_refresh`](crates/lumia_core/src/lambda_lift/abi_refresh/)；只吃 typed cap 表。
- [ ] **`mono/specialize` 与 `ret_ty` 未共享 lattice**：相位拆分（collect/rewrite/ret_refresh/forwarders/funref）已落地；`SigShadow` 已消空 `Block` `mem::replace`。仍欠共享 lattice。
- [x] **`nsw_iv` 分析已迁 opt**：feature `lumia_opt/nsw-iv`；[`NswIvPass`](crates/lumia_opt/src/nsw_iv/mod.rs) 写 `CoreFun` sidecar；codegen 只 `install_nsw_from_fun` + 本地 `leaf_defs`。非负字面量 +/\*、**const-upper IV +/\* 字面量**、emit `nuw` 已加。仍欠更广开放循环 NSW。
- [ ] **SSA `Local` + 字符串 `Name`/`Assign` 双寻址**：ABI/`slot_tys` 双轨；宜槽位统一 `Local`/`SlotId`。
- [ ] **`InferValueCtx` Option 表 / `FunTables` 仍厚**：`ModuleTables` + `FunTables::seed_abi_from` 已消手抄。仍自建 LLVM 句柄与 `closure_cap_tys`/`adt_show_kinds`；`fun_index` 仅 mono。
- [x] **Builtin→RT 符号已并入 `BuiltinInfo`**：`string_receiver_rt` / `list_receiver_rt`；codegen 薄委托。
- [x] **HIR `for_each_expr_skipping_lambdas` + `fun_body_has_io`**：visit 跳过 Lambda 体；effects 与 skip-lambda 子树布局对齐（Call/Let/Builtin 仍专用 walk 跟 Fun 效应）。
- [x] **RT FFI crate 级 `not_unsafe_ptr_arg_deref` allow 已删**：子系统 `unsafe extern "C"` + `deny` + `# Safety`。
- [ ] **CI/check 仍 `clippy`/`test --exclude lumia`**：`lumia`/`lumia_core` clippy 债未收；slim 冒烟已接 `check.sh`/CI，勿回退。

### 续（2026-08-16 第三轮）
#### IR / 类型层
- [ ] **树形 Core 冒充 SSA，无 CFG**：`Value::{If,Loop,Lambda}` 嵌整块；无基本块图 → 每 pass 自写嵌套 walker。
- [ ] **中端仍吃开放 `lumia_ty::Type`**：无封闭 `CoreTy` ABI lattice；哨兵 `Var(u32::MAX)` 仍在。
- [ ] **效应三套真源**：`ty::Effect` / `BuiltinEffect` / `Op::Let.pure_region` + 事后 `effects` 审计；opt 可不绑 `CoreFun.effect`。
- [ ] **`Scheme` 假类型类袋**：9 套平行 `*_vars` 与 `trait_preds` 并列；宜统一谓词 IR。
- [ ] **`match` 在 typing 前擦成 If**：HIR 无 Match；穷尽性仍吃 syntax；诊断无法挂 typed Match。
- [ ] **trait/instance 塌成字符串旁表**：无结构化 TraitDef；UFCS 与 mono stub 易脱节。
- [ ] **表面无类型 AST**：注解/FFI 皆 `String`；宜 syntax 产出 `TypeExpr`。
- [ ] **Span 死于 Core**：中后端诊断多为无位置 `String`；`type_at_span` 线性戳表。
- [ ] **`BuiltinInfo` 非类型规则真源**：真实规则在 `ty/infer/builtins/**` 手写。
- [ ] **结构化并发在 HIR lower 抹平**：`scope`/`spawn`→builtin；ty/opt 不见作用域括号。
- [ ] **HOF/`for` 大量预类型脱糖**：融合形状不可经类型回收。
- [ ] **积/和双声明、单一 `Type::Adt`**：字段/`with`/Show 永特判。
- [ ] **`CoreModule` 是分析黑板**：元数据所有权与「何时权威」不清；宜不可变模块 + `AnalysisFacts`。

#### 中端 / codegen / RT
- [ ] **编译选项仍四散**：CLI 已有 [`CompileOptions`](crates/lumia/src/build.rs)；中端/codegen 仍分 `OptOptions`/`CodegenOptions`；无 codegen feature 时 check 直调 `check_program`。
- [ ] **C vs Runtime marshalling 表仍双份**：用户函数仍统一 i64；foreign 已由 `ForeignAbi` 驱动。
- [x] **`emit_fun` 已拆 prologue + block/tco/cow/let_bind**：`mod.rs` ≈203 编排。
- [x] **领域 SR 批量迁出 opt（batch 2）**：affine2 / gcd / divisor / product-rem / range-affine1 / matmul / mandelbrot whole-fn → `lumia_opt/domain-sr`；codegen 仅留 `collatzSteps` cttz、trial-div odd-step、`floatOrbit` IR。
- [ ] **Task ↔ GC ↔ list-par 硬耦合**：已收窄（`forbid_list_parallel`、rooted publish、栈 freelist）。宜继续抽 shade 算法边界。
- [ ] **`lumia_opt` 第三前端入口**：`compile_source_to_optimized*` 仍跳 loader/std（fixture-only，已有锁测）。

#### 工具链 / 文档 / 测试
- [ ] **import 整模块内联、无编译单元边界**：无增量编译、无库 ABI。
- [ ] **LSP 进程级 `Mutex<State>` + Full sync only**：无 multi-root。
- [ ] **LSP 功能测跳过 loader**：核心面已有 `*_via_loader`；其它面仍可补。
- [ ] **IDE Run/Check 走 CLI shell，分析走进程内 `check_program`**：两套入口、两套 flag。
- [ ] **正确性门四套并行**：e2e / `opt_correctness` / `golden_core` / RT stress；宜一条程序管线测 + 分层夹具。
- [x] **`bench_cn_*.sh` / 本地 bench 骨架已抽 [`bench_measure`](scripts/bench_measure.sh)**：cn 同构脚本与 vs_torch Lumia 侧已瘦身。

#### 补遗
- [ ] **`Type`/`Effect` 住在 `lumia_ty`**：`lumia_core` 硬依赖推断 crate；宜类型定义与推断分 crate。
- [ ] **和类型 `sum_max_arity` 垫统一 `params`**：异变体载荷共享类型变量的表示根因（Show/Eq 症状已缓解）。
- [ ] **`lambda_lift` 名不副实**：`abi_refine` 门面已加；`float_abi` 体量/迁目录仍欠。
- [x] **`run_core_abi_pipeline` 已迁出 lower**：[`abi_pipeline.rs`](crates/lumia_core/src/abi_pipeline.rs)；`lower` 只做 HIR→Core。
- [ ] **Escape / Lit\* repr 所有权骑 core↔opt**：opt 前 Core「合法但不完整」。

### 续（2026-08-16 第四轮）
#### 前端 / 类型 / 诊断
- [ ] **表面糖在 parser 抹平**：fmt/LSP 已回打 `..`/`to`/裸 `it`/尾随闭包。仍欠 typed/HIR 脱糖阶段。
- [ ] **仅 item 级恢复**：无表达式级恢复；一处坏表达式可吞整项。
- [ ] **`bump`/`Checkpoint` 仍可分配**：`bump` 已 `mem::replace`；Ident/`String` 字面量与 Checkpoint clone 仍欠 intern/arena。
- [ ] **一切积/和盲插 `Eq`/`Show` instance**：非 langitem/注册表派生。

#### RT / opt / codegen
- [ ] **三套互不兼容的「持久更新」模型**：List/ADT RC-COW；Map/Set Overlay（Set 已对齐 Map）；**共享 delta 壳**已落地（List patch 同用，2026-08-18）；仍欠共享 materialize。List Iota 稀疏 patch 已加。
- [x] **Map/Set 开哈希近克隆已抽** [`hash_probe`](crates/lumia_rt/src/map_set/hash_probe.rs)。
- [ ] **Memo 存 TLS、堆是进程全局**：OS worker 间不共享命中。
- [x] **Memo 规划已认 `IndirectCall`/FunRef 自递归**。
- [ ] **Escape 摘要仍按名解析 Call**：存储已按 `EscapeFunId`；宜 Call/FunRef 携带稳定 FunId。
- [ ] **目标三元组仍锁宿主**：欠交叉编译与「只出对象不链」。
- [ ] **workspace Inkwell 钉死 `target-x86`**：非 x86 宿主结构性出局。
- [ ] **`lumia_rt`/`opt`/`core` 无 Cargo feature**：领域核/SIMD/stress 无法包级裁剪。
- [ ] **RT 测例半迁**：多数子系统测已外置；仍可再压 common 生产侧。
- [ ] **`examples/` 扁平回归堆**：≈244 顶层 `.lm`；宜 `examples/{guide,reject,bench,task}`。

#### LSP / 包 / 编辑器 / CLI
- [ ] **LSP 诊断缺 relatedInformation/tags**：`Warning` severity 已加；仍欠 tags 与更多 soft 种类。
- [ ] **LSP 能力面缺口大**：无 references/rename/signatureHelp/codeAction/…；`initialize` 忽略 client capabilities。
- [ ] **`pkg` 仍仅 init/lock/add**：缺 update/remove/outdated。

### 续（2026-08-16 第五轮）
- [x] **`float_cap_fixup` 已拆为 [`abi_refresh/`](crates/lumia_core/src/lambda_lift/abi_refresh/)**。
- [ ] **`ModuleTables` / `FunTables` 半收口**：播种已统一；`float_abi` 参数面 / `closure_cap_tys` 计算仍可继续迁。
- [ ] **`lambda_lift/heap.rs` 第四套「是否堆」**：`ResultHeap` + stamp 已对齐 codegen。仍欠 typed 表驱动 + 与 float_abi/mono 合流。
- [ ] **`runtime_decls.rs` 手维百科 ≈1293 行**：宜生成/diff 或按子系统拆表并 CI 对账。
- [x] **`scheduler.rs` 假拆分已收口**：queue/cancel/roots/resume + `home_coro`/`scan_ptrs`；主文件 ≈TLS/freelist/FFI/再导出。

### 续（2026-08-16 第六轮）
- [ ] **HIR 脱糖合成名成第六套命名协议**：[`desugar_slots`](crates/lumia_hir/src/desugar_slots.rs) 已收口真源。仍欠 `LocalKind`/`SlotRole`，禁止中端解析字符串。
- [x] **Inline 再引入 `$inl{tag}_` 槽名**：已改为 **`$s{id}` / 前缀保留**（同 Local `next`，2026-08-18）。仍欠 `Assign`/`Name` 真 Local 化。
- [ ] **双轨函数特化：类型 mono × 常量 specialize**：阶段分界已 lock-in；宜统一 Specialization 框架。
- [ ] **未知类型普遍 `unwrap_or(Type::Int)`**：宜显式 `CoreTy::Unknown`。
- [ ] **mono 近距测仍可补**：`specialize` 生产已拆；`traits`/`rewrite` 仍可再压。
- [x] **`extras.linalg` + `std.linalg` 弃用 shim**：实现在 `extras/linalg.lm`；`std/linalg.lm` 再导出。仍欠 RT 域核 Cargo feature。
- [ ] **RT `dispatch.rs` = 开放方法运行时孪生**：与 ty `*_vars` 同族语义分属两处。
- [ ] **前端巨型分发入口**：`infer_module_inner` / `hir/lower_expr` / `parse_primary` 宜按族拆文件。
- [ ] **LSP format 仍二次严格 `parse_module`**：契约已明确（禁止复用 recovering AST）；可选缓存严格树。
- [ ] **工作区级 clippy allow 仍宽**：已收窄到 crate 顶；宜继续下沉到模块。

### 续（2026-08-16 第七轮）
- [x] **`SigShadow` 取代空 `Block` `mem::replace`**：[`signature_shadow`](crates/lumia_core/src/mono/fun_index.rs) / `SigShadow`；`traits` 与 `specialize/rewrite` 原地改活体。
- [ ] **RT 全局初始化三轨**：[`globals`](crates/lumia_rt/src/globals.rs) 契约表已扩。其它实现仍分散；新全局只经此表登记。
- [ ] **编辑器门禁仍半边**：仍欠 IDEA 缩进/注释契约、非 Linux CI、矩阵测。
- [ ] **opt/mono 测试密度仍不均**：多 pass 测已外置；其它 pass / specialize 等仍偏薄。

### 续（2026-08-16 第八轮）
- [ ] **「是否堆」多套启发式未完全合流**：codegen `type_may_heap` 已薄封装 Core；共享 [`builtin_result_may_heap`](crates/lumia_core/src/value_ty/mod.rs) + [`HeapMay`](crates/lumia_core/src/value_ty/mod.rs)（rooting vs slot）。`mono/key::type_is_mono_container` 有意不含 String/Fun。仍欠其余 Builtin / `ret_ty` 完全合流。
- [x] **`emit_value_if` 根状态**：已改为入口 `HashMap` 快照 + 整表 restore（删 `Rc` COW 三路启发式）。

### 续（2026-08-16 第十二轮）
- [x] **`SchedCore: Send` 覆盖 `!Send` Coroutine**：栈在 `home_coro`；扫描指针在 `scan_ptrs`；自然 `Send`。
- [ ] **根分析「未知→非堆」假收口**：[`HeapMay`](crates/lumia_core/src/value_ty/mod.rs) 已文档化双投影（Unknown→root / Unknown↛slot）。真统一仍需显式 `CoreTy::Unknown` lattice。
- [x] **`gc/` 压力原子归属** [`pressure`](crates/lumia_rt/src/gc/pressure.rs)。
- [ ] **codegen `emit_value/builtin/` 仍厚**：[`convention.rs`](crates/lumia_codegen/src/emit_value/builtin/convention.rs) 已抽出表驱动 emit；`show`+`task` 仍可继续拆。
- [x] **codegen Show/Eq/Println 特化符号汤已收口**：`ShowForm` + `SPECIALIZED_*_RT` + `call_trait_override`；嵌套 Show/Bool TID/immortal empty/par_map Float→Int 等 BUG 已修（见 git）。
- [ ] **`emit_arith` 算术厨房**：已拆 `checked`/`ops`。仍可再压 Num ICE / 与 `nsw_iv` 边界。
- [ ] **`mono/tests` 外置成测沼**：已再拆子目录；子模块近距测仍可补。
- [x] **`fiber.rs`↔`scheduler` 责任边界已抽** [`sched_fiber_api`](crates/lumia_rt/src/task/sched_fiber_api.rs) + `home_coro`。
- [x] **opt `inline.rs` `$inl` 协议**：已废止 `$inl{tag}_`；槽名走 `$s{id}` + 脱糖前缀保留（2026-08-18）。仍欠 IR 级 Local 槽。

## 已落地记录

历史收口与功能完成项详见 git 历史与 [docs/BUILD.md](docs/BUILD.md)。本文件 `[x]` 为经复核已闭环、暂留打叉备查的条目。

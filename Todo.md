# Lumia — 本轮未完成 / 待推进

记录经查实、尚未完整修复或需更大设计落地的问题。语义以 [docs/DESIGN.md](docs/DESIGN.md) 为准；分期见 [docs/BUILD.md](docs/BUILD.md)。
已确认落地的历史项见下方 `[x]` 与 git 历史。

## 性能优化清单（2026-08-17）

面向 **编译更快 / 跑得更快 / 更省内存**。与下方「性能债」「架构卫生」交叉处只写动作与优先级，细节仍以原条目为准。

### 编译速度（主机侧：解析 → 中端 → LLVM → 链）

- [x] **Token / Ident 实习 + 少 clone**（高）：**Lexer span-only**（`TokenKind::Ident` + `StringPart::{Ident,ExprSrc}`）；**Parser `StringInterner` + AST `Sym`（`Arc<str>`）** 解析期去重 ident **与字面量 `String`**；**HIR 持 `Sym`**（user 名 + 字面量）；Core 边界 `Sym→String`（2026-08-19）。
- [ ] **前端 arena（syntax/HIR）少拷 Expr 子树**（高）：脱糖与 `list_hof` 大量 `body.clone()`；大模块峰值 RSS 与解析 CPU 同涨。
- [ ] **import 增量编译单元**（高）：整模块内联重解析/重类型/重 opt/重 codegen。无 ABI 边界则大工程与 LSP 编译墙钟无解。← 与「import 整模块内联」同债。
- [x] **Memo plan 单次扫模块**（高）：[`memo/plan.rs`](crates/lumia_opt/src/memo/plan.rs) 一次 `max_const_arg_reuse_by_fun` 收集 `fun → const-arg 最大频次`；`slots_cost_ok` 只查表（原每候选全模块扫 ≈O(n²)）。
- [x] **Escape 固定点降本**（中高）：worklist + Call 图 reverse edges；预算耗尽只强制**仍开放 SCC**（非整模块）全参逃逸（2026-08-18）。**`Call`/`FunRef`/`AllocClosure` 均 `CallTarget`+`FunId`**；Escape 入口 `resolve_module_call_fun_ids`（2026-08-18）。
- [x] **中端少 clone `CoreFun` 体**（中高）：inline / specialize_const / **mono（含 FunRef directize）均已 `Arc` 体模板**（2026-08-18）。Inline 槽名改为 **`$s{id}`（同 Local 计数器）** 并保留脱糖前缀；仍欠真 `Assign`/`Name`→Local（2026-08-18）。
- [ ] **树形 IR → CFG 或统一 visitor**（中）：每 pass 自写嵌套 walker；CFG/`visit` 默认入口降中端编译时间与漏扫。← 与「树形 Core」「visit 未成默认入口」同债。
- [x] **Release LLVM 二次 `verify` 可闸**（中）：[`codegen/lib.rs`](crates/lumia_codegen/src/lib.rs) emit 后仍 verify；O3 后再验需 `LUMIA_VERIFY`（见 BUILD §8）。
- [x] **workspace `[profile.release]`**（中）：根 `Cargo.toml` thin LTO + `strip = "debuginfo"`；`lumia_rt` `codegen-units = 1`。
- [x] **默认/文档推 `llvm-dynamic` 链编译器**（中）：`lumia` `default = ["codegen", "llvm-dynamic"]`；裸 `cargo build -p lumia` 走共享 libLLVM。Windows / 静态：`--no-default-features --features codegen`（CI/`check.sh` 已分叉）。BUILD §4.2 已同步（2026-08-18）。
- [x] **Debug/check 可选用 LLVM `-O1`/`fast`**（低中）：[`LlvmOptLevel`](crates/lumia_codegen/src/opt_level.rs) 与 `--llvm-opt={none,1,2,3}`（`fast`=`1`，非 `-Ofast`）。Debug 默认 `default<O1>`，`--release` 默认 O3；中端 Debug vs Release 仍只看 `--release`。`--emit-llvm` 为管线后 IR（2026-08-18）。
- [x] **字符串键 → FunId / 实习名**（中）：`Call` / `FunRef` / `AllocClosure` 已统一 [`CallTarget`](crates/lumia_core/src/ir.rs)（`name: Sym`+可选 `FunId`）；**`optimize` 首尾 + Escape 入口 + [`run_core_abi_pipeline`](crates/lumia_core/src/abi_pipeline.rs) 出口** `resolve_module_call_fun_ids`（2026-08-19）；**mono `FunRefKey` 填 id 统一走 [`FunIndex::stamp_funref_ids`](crates/lumia_core/src/mono/fun_index.rs)**（2026-08-19）；**syntax `Sym` intern 已落地**（2026-08-19）；**Core IR `Op::Assign`/`Value::Name`/`param_names`/`CoreFun::name` 已迁至 `Sym`**（2026-08-19）。**命名槽位表全量 `Sym` 键**（`slot_tys` / `float_slots` / `bool_slots` / `seen_slots` / mono `slot_*_funrefs` / codegen Frame `slots`·`rooted_slots`·`slot_i64_const`；`Sym: Borrow<str>`；escape/inline/memo/specialize_const/nsw_iv 槽与函数名映射跟进，2026-08-19）。**函数 ABI 旁表 + CallTarget + codegen 槽 API 已迁 `Sym`**（`ModuleTables`/`FunTables` `fun_ret_tys`·`fun_param_tys`·`fun_param0_identity`；lambda_lift/mono/abi_refresh/channel_hint/float_abi；codegen `ensure_slot`/`store_slot`/`load_slot`/`current_fun`/`closure_cap_tys`，2026-08-19）。**ADT 名旁表已迁 `Sym` 键**（`hash_adts` / `adt_variant_names` / `sum_max_arity` + `ModuleTables`/`FunTables`/`InferValueCtx`/`adt_show_kinds`；变体 display 标签仍 `Vec<String>`，2026-08-19）。**trait/mono rename 已迁 `Sym`**（`trait_methods: HashMap<(Sym,Sym),Vec<Sym>>`；mono `renames`/`needed`/`forward`/`join_*_funrefs`；`CoreFun::mono_of: Option<Sym>`，2026-08-19）。**io_funs / HofSets / lifted lambda 名集已迁 `Sym`**（lower + lambda_lift；`float_cap_idxs` / escape `name_to_id` / `FunRefAliases` 此前已 `Sym`，2026-08-19）。**`Type::Adt.name` / `AllocAdt.adt_name` / `MonoKind::Adt` / codegen `FunTables.functions` 已迁 `Sym`**（ty infer `ProductState`/`TraitState`/`ufcs_rewrites`；HIR `adt_classify`；codegen `adt_method_name`，2026-08-19）。仍刻意保留 String：`CoreFun.external`（C 符号）、LSP URI 表、ADT 变体 display 字符串；推断 `fun_types`/`env`/`trait_preds` 等仍 `String`（非本项范围）。
- [x] **`Type` 树实习 / 封闭 `CoreTy`**（中）：推断开放 `lumia_ty::Type`（复合节点 `Arc`）；`Type::Unknown` 显式洞；HIR→Core `close_type` 关闭 vars/prefix/effect；`HeapMay`/`TCO`/`lower` 跟进（2026-08-19）。
- [x] **LSP 脏模块缓存**（中）：`ProgramCache` 按 `(ide_entry, overlay_fp, auto_parallel)` 跳过重复 load+typecheck；`format_cache` 按 source hash；watched files / `autoParallel` 变更失效（2026-08-19）。

### 运行速度（生成码 + RT）

- [x] **Iota 虚拟列表落地**（高）：RT `TYPE_LIST_IOTA`（`lumia_range`）O(1) 建表；get/len/take/slice/par_map 走虚路径；`set` 叠 patch；**相邻 Iota concat 仍虚**；unique reverse/sort/sortBy/concat 吃 spare。Core `ListRepr::Fused` 仍为 HIR 脱糖保留标签（ReprSelect 不发出）；逃逸管道按普通 `map`/`filter` 物化（2026-08-18）。
- [ ] **HOF 融合超出 fold 汇合**（高）：build/`flatMap`/`any`/`all`/`find`/`len`/`isEmpty`/`contains`/`toSet`/`toMap`/`toList`；**`for-in` 扫 map/filter/take/drop/flatMap**；Let-bound for-in / contains / get/len/take/drop/**isEmpty** / **any** / **all** / **find** / **assoc fold**；take|drop×消费端；**`toMap().get/contains` 单键扫配对流（last-wins Option；≥2 次仍建 Hash）**；**`toSet().contains` 短路扫**；Loop 内 get 不脱糖（2026-08-18）。**Let-bound `isEmpty` / `take.isEmpty` / `drop.isEmpty` deforest**；**`drop(n).isEmpty` skip 后短路**；**Let-bound `ListParFold` 降序后走 for-in 融合**；**Let-bound nested `take.take`/`drop.drop`（min/sum）+ `drop.drop.isEmpty`**；**pipe `drop.drop.isEmpty` / `take.drop.isEmpty` skip 后短路**；**Let-bound `take.drop` 单 builder + `take.drop.isEmpty` 短路**（2026-08-19）。**iota+lone map/filter+len(±gets) 保物化（ListParMap / __flt_acc；非 shared-scan）**（`range_map` golden 回归，2026-08-19）。
- [x] **扩大 NSW/`nuw` Int 算术**（高）：默认 `llvm.*.with.overflow`；`nsw_iv` 已覆盖 IV/树/字面量 + const-upper 非负 IV `+`/`*` + **开放排他 `i < n` 最坏 `U=MAX-1`（仅 `i+1`）** + **named-upper 的开放 inclusive `i <= n`（嵌套在已有 `iv_upper` 下）** + **safe Div / 小 const Rem 不依赖 IV 界** + **有界非负 IV 对加/乘 `i+j` / `i*j`**（2026-08-18）。仍欠更广一般算术。
- [ ] **更富内联**（高）：`INLINE_MAX_OPS=64`（Domain SR 已覆盖 `$c_` 克隆后恢复；2026-08-18）；`IndirectCall`+`FunRef` 已可内联。仍欠热度、捕获闭包栈分配 / defunctionalize；仅 Release。
- [x] **GC 根跨块 last-use**（高）：同块无 safepoint 跳过 `root_push` 已落地；**纯 `If` 臂 / 夹心纯 `If` / GC-free `Loop`（含体内用途）** 已纳入 last-use 消根；**有 safepoint 时 last-use 早 `root_pop`（含 `lumia_root_swap_remove` 非栈顶死根）**（2026-08-19）；**`CrossBlockLastUse`：`If` 单臂 + `Loop` exit + `Lambda`/`AllocClosure` exit 早 pop + checkpoint 刷新**（2026-08-19）。**AdtField（base rooted）→ Call 实参 / 只读 builtin 接收者** skip retain；**ephemeral 用途扫描对齐 If 单臂**（2026-08-19）。
- [ ] **未逃逸闭包 / 小 ADT 栈化**（高）：escape 已有，物化仍偏堆；DESIGN 栈/SROA 路径未吃满。
- [ ] **通用 `List[Float]` 向量化**（中高）：list-out 已统一 `var out = dest`（[`OutSlot`](crates/lumia_opt/src/dense_f64_sr/shape_util.rs)；elem/BLAS/norm 同入口；fill/copy/set 用槽约束，2026-08-18）。`val out = dest` 不能当写目标。未匹配仍标量 SSA + RT list。
- [x] **Set 走 Overlay 类持久更新**（中高）：Map 同款 `[-1][parent][dn][e…]`；Hash `insert` 叠 delta（≤`SMALL_CONTAINER_MAX`）再 materialize；mark/evacuate/show/eq 已接（2026-08-18）。
- [x] **Map/Set 唯一 RC 原地更新**（中高）：Map/Set alloc `rc=1` + COW；unique overlay 更新/追加（壳按 `OVERLAY_MAX` 预分配）；unique Hash 在负载允许时原地 upsert；**线性表预留容量后 unique 追加**；Set 已存在元素 identity；**remove 未命中 identity**（含 overlay）；**unique 线性 compact**；**unique Hash tombstone 删除**（`n > SMALL_MAX`）与 **原地 demote 线性**（`n ≤ SMALL_MAX`）；**overlay 仅 delta 键 remove 不 materialize**（父键命中仍 flatten）；**codegen `s = s.insert/remove` 与 `xs = xs.reverse/sort/sortBy` 走 COW consume**（2026-08-18）。
- [ ] **Map/Set `Small*` / `BuildFused` / 单键短路**（中）：HIR 已对未逃逸 **单次** `pipe.toMap().get/contains`、`pipe.toSet().contains` 做线性扫（2026-08-18）；仍欠运行时 `SmallMap` / 逃逸后补建哈希。
- [ ] **进程共享 Memo `T_f`**（中）：现 TLS-only；OS worker 间不共享命中。
- [x] **领域 SR 迁出 codegen → opt**（中）：batch1+2 whole-fn 已迁；**trial-div odd-step 已迁 Core**（`TrialDivOddPass`，Debug+Release）；**`collatzSteps` cttz 已迁 RT + `CollatzStepsPass`（Debug+Release）**（2026-08-19）。**`floatOrbit` 已迁 RT + `FloatOrbitPass`（Debug+Release）+ `DomainSrPass`（Release；删 codegen `<4|8 x double>` IR emit，2026-08-19）**。**`memTrafficChecksum` 已从 release-only `DomainSrPass` 额外拆为独立 `MemTrafficPass`（Debug+Release）**，并将 `match_mem_traffic` 从 bench 聚合 matcher 解耦为专用入口（2026-08-20）。**0 参 `$c_` 克隆**可命中；Release 管道在 `SpecializeConst` 前/后保留 whole-fn SR；**domain_sr / dense_f64_sr 形状 peep 已统一 [`sr_pattern.rs`](crates/lumia_core/src/sr_pattern.rs)**（pre-loop slot / outer bound / OutSlot / block_calls_any；删 `domain_sr/util.rs`，2026-08-19）。
- [ ] **List-par 与 GC 再解耦**（中）：调度/根/nursery 边界继续收窄，降并行 HOF 停顿。
- [x] **TCO 覆盖面审计**（低中）：补齐 `lumia_core/src/tco.rs` 的边界回归：
  - `tco_scc_excludes_fun_typed_ret`：`Type::Fun`（函数值/闭包值）从纯 TCO SCC 中彻底排除
  - `resolve_tco_callee_rejects_local_alias_cycle`：local alias 环应返回 `None`（防止递归发散）
  - `resolve_tco_tail_call_indirect_via_local_alias_chain`：更长的 SSA `Local` 别名链最终剥离到 `IndirectCall`，仍能解析到正确 peer
  验证：`cargo test -p lumia_core --lib` 全绿（231/231 通过）。

### 内存占用（编译器峰值 + 程序运行）

- [ ] **解析/HIR arena + 少 Expr clone**（高）：同上前端 arena；直接压编译器峰值。
- [x] **少根槽 / 更短 live root**（高）：跨块消根 + **last-use 早 pop + swap_remove 非栈顶死根** + **`CrossBlockLastUse`（If 单臂 / Loop exit / Lambda exit）** + **AdtField rooted-base ephemeral**（2026-08-19）后 nursery mark 扫描更短、停顿更小。
- [x] **inline/mono 避免整函数体 clone**（中高）：inline + specialize_const + mono FunRef 路径均 `Arc` 模板（2026-08-18）。
- [ ] **统一持久容器层**（中高）：List/ADT RC-COW vs Map/Set Overlay；**delta 壳**（nbytes/parent/dn/clamp/mark-parent）已抽 [`container_delta.rs`](crates/lumia_rt/src/container_delta.rs)，Map/Set Overlay + List patch + GC 已接（2026-08-18）。仍欠共享 materialize / 全量合流。
- [ ] **ReprSelect / Lit\* 推更多栈与永生小值**（中）：空 List/Map 永生已有；小未逃逸 ADT/Map/Set 栈路径仍欠。
- [ ] **Nursery / TLS LAB 按负载可调**（中）：已有分代+LAB；LAB 尺寸与 young limit 可工作负载调参或自适应。
- [x] **DCE 收紧 `builtin_may_trap_or_effect`**（低中）：`Range`/`RangeInclusive`/`AdtTag` 已移出陷阱集；DCE 已迭代到不动点以清掉死 Builtin 的残留参数（2026-08-17）。**DCE/LICM/CSE 共享 [`builtin_effect.rs`](crates/lumia_opt/src/builtin_effect.rs)**（2026-08-19）。
- [x] **链时 `--gc-sections` + RT thin LTO**（低中）：link 已 gc-sections；workspace release thin LTO + `lumia_rt` CGU=1 已落地（2026-08-17）。
- [ ] **`--mm=arc` 仍非近优**（低）：分代 GC 已主路径；ARC 仅为延迟敏感愿景。

### 建议推进顺序（ROI）

1. **跑得更快**：HOF 续（`.get`/Let 脱糖 + **toMap 单键扫**已落地；Iota 仍 RT）→ 广 NSW → 少 GC 根（**swap_remove 非栈顶早 pop** 已落地）→ 闭包/小值栈化；Set overlay 已落地。
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
- [x] **堆类型 Let 默认 `root_push`**：ephemeral + 同块无 safepoint 的 `let_skip_root_no_safepoint` 已落地；**纯 `If` / GC-free `Loop` 嵌套用途与夹心** 已消根；**有 safepoint 时 last-use 早 pop（含 swap_remove 非栈顶）** + **`CrossBlockLastUse` 跨 If/Loop/Lambda 早 pop** + **AdtField→call 且 base 已 rooted 时 skip retain/root**（2026-08-19）。Lambda 体内用途仍保守 retain（skip-root 拒绝）。
- [ ] **通用 `List[Float]` 向量化靠 `dense_f64_sr`**：out-slot 协议已统一（2026-08-18）；未匹配形状仍标量 SSA + RT list；非通用向量管线。
- [x] **Memo plan 单次扫模块**：`max_const_arg_reuse_by_fun` 一次收集 reuse（2026-08-17）。
- [x] **Escape 固定点仍贵**：已改 worklist + 开放 SCC 强制（2026-08-18）；**Call 已挂 `FunId`（`CallTarget`）**（2026-08-18）。

## 工具链

- [ ] **`--mm=arc` / 可插拔 GC 仍非优先**：分代 STW minor + 增量并发 full mark 已落地；ARC 后端仍为愿景。

## 架构卫生

### 结构 / 一致性（仍欠）
- [ ] **Core Float ABI / `local_heap_ty` 仍厚**：`float_abi/` 已按相位拆分，`prefer`/`join` 已迁 `value_ty/join`。仍欠与 `value_ty` / `mono/ret_ty` 整 walker 合流。
- [x] **领域 SR 仍侵入 codegen + RT**：batch1+2 whole-fn + trial-div odd-step + **collatzSteps RT** + **floatOrbit RT** 已迁 `lumia_opt/domain-sr`（2026-08-19）；**RT 域核已加 Cargo feature `lumia_rt/domain-sr-rt`（默认开，可 `--no-default-features` 裁剪）**并门控 `collatz` / `float_kernels` / `mem_traffic`（2026-08-20）。验证：`cargo check -p lumia_rt` 与 `cargo check -p lumia_rt --no-default-features` 均通过。
- [x] **`dense_f64_sr` opt 侧门闩**：Cargo feature `dense-f64-sr`（默认开）；`lumia` `codegen` 开启、`codegen-slim` 关闭。仍欠 codegen 领域 SR 分层。
- [ ] **Core IR 穿透携带 `lumia_hir::Builtin`**：中后端必须依赖 `lumia_hir`+`lumia_syntax`；宜 Core 自有 opcode/元数据。
- [ ] **自动并行决策跨 HIR→ty 两阶段**：`list_hof` 先升 `ListPar*`，`finalize_auto_parallel` 再 demote；策略散在前端两层。
- [ ] **跨层错误类型分裂**：`LocatedError` / `TypeError` / `Result<_, String>` / `anyhow`；诊断易丢 span。
- [ ] **库路径 panic vs Result 不一**：`lumia_ty` alt/PRELUDE_CTOR 已改 `TypeError`/`try_new`；`lumia_core` lower 等对「理论不可达」仍可有 `expect`（非 test）。
- [x] **双前端管线分叉**：`lumia_core::compile_typed_to_core` 已收口共享 **typed→Core→ABI/channel** 段；`lumia::compile_program_to_core` / `compile_program_to_optimized` 提供 **loader+std** 真入口；CLI `build` 已复用同一路径（2026-08-19）。保留 `compile_source_to_core*` / `compile_source_to_optimized*` 作为**单缓冲 fixture helper**，但不再冒充完整程序入口。
- [ ] **`visit.rs` 未成默认入口**：共享 `for_each_*`/`collect_*`/`body_weight` 已扩；**`for_each_let_in_block_ctrl`（If-fork + Loop 顺序 env）**、**`for_each_top_level_op_in_block_mut`**、**`flat_map_top_level_ops_in_block`（take/splice 重组）** 等已加；**memo plan / specialize_const / domain_sr / dce / copy_elim / repr_select / inline / lambda_lift rewrite+captures / mono directize+traits+forwarders+collect / funref_alias / memo licm+cse+fold / dense_f64 gemv / sr_pattern collect_leaf_defs / trial_div_odd / escape seed+propagate** 已迁 visit 入口（2026-08-19）。**`for_each_op_in_block_mut`** 已加（2026-08-19）。**`*_sr` 形状 peep 已收口 [`sr_pattern.rs`](crates/lumia_core/src/sr_pattern.rs)**。**SSA def 查找收口 [`find_local_def`](crates/lumia_core/src/visit.rs) / [`find_top_level_local_def`](crates/lumia_core/src/visit.rs)**（删 `let_value_dfs` / **`let_value` 第四份**；`local_lookup` / `float_abi/helpers` / `heap` / `channel_hint` / `float_caps` / `rewrite` / **`mark_float` 顶层 chase** / **`mono/ret_ty` slot+const 查找** / **`ret_refresh` AllocClosure 别名** 已迁，2026-08-19）。**`mark_float_uses` / `compute_float_locals_from` 已迁 `for_each_top_level_op_in_block`**（`defs` 改 owned `Value` 解除 E0521，2026-08-19）。**block result 别名剥离收口 [`peel_block_result`](crates/lumia_core/src/visit.rs) / [`peel_local_to_value`](crates/lumia_core/src/visit.rs)**（`block_result_is_bottom` / **`block_result_is_bool_lit`** / `block_result_is_unit` 共享，2026-08-19）。**命名 slot `Assign` 遍历收口 [`for_each_named_slot_assign_in_block`](crates/lumia_core/src/visit.rs)**（`float_abi/helpers` slot heap join / `mono/ret_ty` 已迁，2026-08-19）。**`channel_hint_tests` 顶层 op 扫描迁 `for_each_top_level_op_in_block`**（2026-08-19）。codegen loop SR 已清空（floatOrbit → domain_sr，2026-08-19）。
- [ ] **Windows 工作流仍薄**：`env.ps1` 已对称 PATH/LIB。仍欠 Nix 级发现与完整 `.ps1` 工作流。

### 续（2026-08-15 第二轮）
- [ ] **Value→Type 三套 walker 未合流**：`builtin_value_ty` / `join_fixed_ty` / gated via / float 薄包装已大幅共用。**命名 slot `Assign` fold 收口 [`fold_slot_assign_ty`](crates/lumia_core/src/value_ty/join.rs) + [`JoinAssignKind`](crates/lumia_core/src/value_ty/join.rs)**（`float_abi/helpers` slot heap / `alloc_elems` / `mono/ret_ty` 统一委托；`join_slot_assign_ty` 仍专管 Fixed 格，2026-08-19）。**If/match 臂合流收口 [`join_if_arm_tys`](crates/lumia_core/src/value_ty/join.rs)**（`float_abi/local_heap` + **`mono/ret_ty` 统一**；bottom 检测收口 [`block_result_is_bottom`](crates/lumia_core/src/visit.rs)，2026-08-19）。仍欠单一 walker（float_abi Float soft vs `ret_ty` 开放 Map 等残留分叉）。
- [x] **`ClosureCap.as_float` 已删除**：旁路 `float_cap_idxs` + [`abi_refresh`](crates/lumia_core/src/lambda_lift/abi_refresh/)；只吃 typed cap 表。
- [x] **`mono/specialize` 与 `ret_ty` 未共享 lattice**：相位拆分已落地；把 `ret_refresh` 里“body fixed ret ↔ mono key inferred ret”的合并决策（`merge_mono_ret_with_inferred` + `option_result_payload_weaker`）迁入 `mono/ret_ty`，由 `mono/specialize` 共享调用，避免同一“ret lattice”在不同阶段分叉实现（2026-08-20）。验证 `cargo test -p lumia_core --lib` 全绿（228/228）。
- [x] **`nsw_iv` 分析已迁 opt**：feature `lumia_opt/nsw-iv`；[`NswIvPass`](crates/lumia_opt/src/nsw_iv/mod.rs) 写 `CoreFun` sidecar；codegen 只 `install_nsw_from_fun` + 本地 `leaf_defs`。非负字面量 +/\*、**const-upper IV +/\* 字面量**、emit `nuw` 已加。仍欠更广开放循环 NSW。
- [ ] **SSA `Local` + 字符串 `Name`/`Assign` 双寻址**：ABI/`slot_tys` 双轨；宜槽位统一 `Local`/`SlotId`。
- [ ] **`InferValueCtx` Option 表 / `FunTables` 仍厚**：`ModuleTables` + `FunTables::seed_abi_from` 已消手抄。仍自建 LLVM 句柄与 `closure_cap_tys`/`adt_show_kinds`；`fun_index` 仅 mono。
- [x] **Builtin→RT 符号已并入 `BuiltinInfo`**：`string_receiver_rt` / `list_receiver_rt`；codegen 薄委托。
- [x] **HIR `for_each_expr_skipping_lambdas` + `fun_body_has_io`**：visit 跳过 Lambda 体；effects 与 skip-lambda 子树布局对齐（Call/Let/Builtin 仍专用 walk 跟 Fun 效应）。
- [x] **RT FFI crate 级 `not_unsafe_ptr_arg_deref` allow 已删**：子系统 `unsafe extern "C"` + `deny` + `# Safety`。
- [x] **CI/check 仍 `clippy`/`test --exclude lumia`**：**`lumia --no-default-features --lib` 全 97 测通过（2026-08-19）**；`check.sh` + `ci.yml` 均加 `cargo clippy/test -p lumia --no-default-features --lib` 步骤；`lsp/inlay/collect.rs`、`lsp/semantic/walk.rs`、`vis.rs` 最后遗留 `Sym`↔`String` 类型错误已修。仍欠：`lumia`/`lumia_core` 完整 clippy（含 codegen feature）。

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
- [x] **编译选项仍四散**：[`CompileOptions`](crates/lumia/src/options.rs) 已迁至 always-available `options.rs`（不依赖 `codegen` feature）；codegen 字段（`link_args`/`emit_llvm`/`llvm_opt`）`#[cfg(feature = "codegen")]`；`compile_program_to_optimized` / `compile_program_to_core` 统一收口 `&CompileOptions`（不再散传 `auto_parallel` + `trust_foreign_pure` + `&OptOptions`）；`CompileOptions::opt()` 委托 `OptOptions::for_build()` 保证 feature 门一致；CLI `check` 不再分叉 `#[cfg(feature = "codegen")]` / 裸 `check_program`，统一走 `CompileOptions`（2026-08-19）。
- [ ] **C vs Runtime marshalling 表仍双份**：用户函数仍统一 i64；foreign 已由 `ForeignAbi` 驱动。
- [x] **`emit_fun` 已拆 prologue + block/tco/cow/let_bind**：`mod.rs` ≈203 编排。
- [x] **领域 SR 批量迁出 opt（batch 2）**：affine2 / gcd / divisor / product-rem / range-affine1 / matmul / mandelbrot whole-fn → `lumia_opt/domain-sr`；**`floatOrbitChecksum` → `lumia_float_orbit_checksum`（删 codegen IR SR，2026-08-19）**。**trial-div odd-step 已迁 Core `TrialDivOddPass`**（2026-08-18）。
- [ ] **Task ↔ GC ↔ list-par 硬耦合**：已收窄（`forbid_list_parallel`、rooted publish、栈 freelist）。宜继续抽 shade 算法边界。
- [x] **`lumia_opt` 第三前端入口**：`compile_source_to_optimized*` 明确降格为 **fixture-only**；loader/std 真入口改由 `lumia::compile_program_to_optimized` 承担，`build` 已走同一路（2026-08-19）。

#### 工具链 / 文档 / 测试
- [ ] **import 整模块内联、无编译单元边界**：无增量编译、无库 ABI。
- [x] **LSP 进程级 `Mutex<State>` + Full sync only**：已完成 `textDocument/didChange` 增量 `range` edits（UTF-8/UTF-16 回归）与 `initialize.capabilities.textDocumentSync={ openClose: true, change: 2 }`；已补基础 multi-root 协商/变更路径（`workspaceFolders.supported/changeNotifications` 打开，处理 `workspace/didChangeWorkspaceFolders`，变更后失效 program cache 并重分析 open docs；含 `parse_workspace_folders_*` / `workspace_folder_change_updates_state` 回归，2026-08-20）；并已完成进程级 `Mutex<State>` 的结构性解耦：将全局 `STATE` 改为“session-local leaked mutex + thread-local session pointer”，`run_lsp` 与 analyze worker 共享同一 session 状态，从而支持多会话并行互不污染（2026-08-20）。 
- [x] **LSP 功能测跳过 loader**：补齐 `references` / `rename` / `codeAction` / `signatureHelp` 的 `*_via_loader` 回归，并把 LSP 测试共享夹具收口到 [`lsp/test_support.rs`](crates/lumia/src/lsp/test_support.rs)（统一 `with_encoding` / imported-alias fixture / open-doc state）。同时为会替换全局 `LSP STATE` 的单测加统一串行门禁，消除 `cargo test -p lumia --lib lsp::` 并发互踩导致的偶发失败；`references` 另修正 loader buffer URI 回映射与 declaration 精确识别（2026-08-20）。
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
- [x] **仅 item 级恢复**：完成“表达式级恢复”。
  - `lumia_syntax` IDE/LSP recovering parse 里，把 block 内 stmt 表达式解析失败改为：记录错误 + 局部插入 `__parse_hole` + 同层同步到下一个 stmt boundary（避免整项被 `parse_val_item_resilient` 吞掉）。
  - match arm/cond arm 的 guard/body 解析失败同样局部降级为 `__parse_hole`，并同步到下一 arm pattern/或 `}` 继续解析。
  - 通过在 `Parser` 内部统一收集 `ParseError`（支持深层恢复回报），使恢复点能在语法树构建阶段工作。
  - 新增回归测试：`recover_recovers_bad_expr_inside_block_keeps_later_stmt`、`recover_recovers_bad_match_arm_body_keeps_later_arms`（`cargo test -p lumia_syntax --lib` 全绿）。
- [x] **`bump`/`Checkpoint` 仍可分配**：`bump` 已 `mem::replace`；**Ident token span-only + AST `Sym` intern（ident + 字面量 `String`）**（2026-08-19）；仍欠 arena。
- [ ] **一切积/和盲插 `Eq`/`Show` instance**：非 langitem/注册表派生。

#### RT / opt / codegen
- [ ] **三套互不兼容的「持久更新」模型**：List/ADT RC-COW；Map/Set Overlay（Set 已对齐 Map）；**共享 delta 壳**已落地（List patch 同用，2026-08-18）；仍欠共享 materialize。List Iota 稀疏 patch 已加。
- [x] **Map/Set 开哈希近克隆已抽** [`hash_probe`](crates/lumia_rt/src/map_set/hash_probe.rs)。
- [ ] **Memo 存 TLS、堆是进程全局**：OS worker 间不共享命中。
- [x] **Memo 规划已认 `IndirectCall`/FunRef 自递归**。
- [x] **Escape 摘要仍按名解析 Call**：存储已按 `EscapeFunId`；**Call/FunRef/AllocClosure 携 `CallTarget`+`FunId`**，`lookup_call` 先 id（2026-08-18）。名字回退仅未 resolve 站点。
- [ ] **目标三元组仍锁宿主**：欠交叉编译与「只出对象不链」。
- [ ] **workspace Inkwell 钉死 `target-x86`**：非 x86 宿主结构性出局。
- [ ] **`lumia_rt`/`opt`/`core` 无 Cargo feature**：`lumia_rt` 已补 **`domain-sr-rt`**（默认开，可 `--no-default-features` 裁剪 domain kernels）；`opt`/`core` 侧仍待补齐更细粒度 feature 面。
- [ ] **RT 测例半迁**：多数子系统测已外置；仍可再压 common 生产侧。
- [x] **`examples/` 扁平回归堆**：已分拆为 `examples/{guide,reject,bench,task}`（guide 184·reject 41·bench 15·task 21）；共享库模块（`math.lm`、`math_priv.lm`）留在包根；loader 新增 manifest 目录加入 `search_roots` 使子目录文件可 import 包根模块；所有 e2e/golden/opt/codegen 测试路径、scripts、CI、BUILD.md、README 已同步更新（2026-08-19）。

#### LSP / 包 / 编辑器 / CLI
- [x] **LSP 诊断缺 relatedInformation/tags**：已为 `DiagnosticKind::Warning` 的 LSP 输出补齐 `tags`（`["Unnecessary"]`）与 `relatedInformation`（advisory hint）；并在 `crates/lumia/src/lsp/analyze.rs` 的按文件 URI 批次阶段回填 `relatedInformation[].location.uri`（2026-08-20）。
- [x] **LSP 能力面缺口大**：已补 `textDocument/references` + `textDocument/rename`，并继续补齐 `textDocument/signatureHelp` + `textDocument/codeAction`（`quickfix: Format document`）；`initialize.capabilities` 已声明 `referencesProvider` / `renameProvider` / `signatureHelpProvider` / `codeActionProvider`（2026-08-20）。新增回归：`lsp::signature_help::{signature_help_add_call_active_param_0,signature_help_add_call_active_param_1,signature_help_imported_alias_via_loader}` 与 `lsp::code_action::code_action_offers_format_document_edits`。
- [x] **`pkg` 仍仅 init/lock/add**：[`remove`](crates/lumia/src/pkg/manifest.rs) / [`update`/`outdated`](crates/lumia/src/pkg/lock.rs) 共用 `write_lock_from_manifest` + `LockDiff`（path 依赖：fingerprint/版本漂移；`outdated` 过期非 0；2026-08-18）。

### 续（2026-08-16 第五轮）
- [x] **`float_cap_fixup` 已拆为 [`abi_refresh/`](crates/lumia_core/src/lambda_lift/abi_refresh/)**。
- [ ] **`ModuleTables` / `FunTables` 半收口**：播种已统一；`float_abi` 参数面 / `closure_cap_tys` 计算仍可继续迁。
- [ ] **`lambda_lift/heap.rs` 第四套「是否堆」**：`ResultHeap` + stamp 已对齐 codegen。仍欠 typed 表驱动 + 与 float_abi/mono 合流。
- [x] **`runtime_decls.rs` 手维百科 ≈1293 行**：已按子系统拆为 16 个 sub-table（Io/Adt/Gc/List/MapSet/String/Compare/Trap/Dict/FloatAbi/DenseFloat/DomainSr/Cn/Memo/Task/Abi）；`rt!` 宏精简声明语法；`RtSubsystem` 枚举 + `every_subsystem_has_decls` 测试保证分类完整；原有 4 项 CI 对账测（唯一性 / dense trampoline / builtin 符号 / RT no_mangle 双向 diff）全通过（2026-08-19）。
- [x] **`scheduler.rs` 假拆分已收口**：queue/cancel/roots/resume + `home_coro`/`scan_ptrs`；主文件 ≈TLS/freelist/FFI/再导出。

### 续（2026-08-16 第六轮）
- [ ] **HIR 脱糖合成名成第六套命名协议**：[`desugar_slots`](crates/lumia_hir/src/desugar_slots.rs) 已收口真源。仍欠 `LocalKind`/`SlotRole`，禁止中端解析字符串。
- [x] **Inline 再引入 `$inl{tag}_` 槽名**：已改为 **`$s{id}` / 前缀保留**（同 Local `next`，2026-08-18）。仍欠 `Assign`/`Name` 真 Local 化。
- [ ] **双轨函数特化：类型 mono × 常量 specialize**：阶段分界已 lock-in；宜统一 Specialization 框架。
- [ ] **未知类型普遍 `unwrap_or(Type::Int)`**：宜显式 `CoreTy::Unknown`。
- [x] **mono 近距测仍可补**：已补 `traits` / `specialize::rewrite` 近距回归（2026-08-20）。
  - `mono/traits.rs`：新增 `traits_near`，覆盖 **`Binary(Add)`→唯一 mangled trait impl Call** 重写，以及 **短名 trait method stub 自动补齐并 `MatchFail` trap**。
  - `mono/specialize/rewrite.rs`：新增 `specialize_rewrite_near`，覆盖 **direct Call 按 `MonoKey` rename** 与 **`IndirectCall(FunRef)` 直化到重命名 clone**。
  - 验证：`cargo test -p lumia_core --lib`（228/228）。
- [x] **`extras.linalg` + `std.linalg` 弃用 shim**：实现在 `extras/linalg.lm`；`std/linalg.lm` 再导出。RT 域核 Cargo feature 已落地（见 `lumia_rt/domain-sr-rt`，2026-08-20）。
- [ ] **RT `dispatch.rs` = 开放方法运行时孪生**：与 ty `*_vars` 同族语义分属两处。
- [ ] **前端巨型分发入口**：`infer_module_inner` / `hir/lower_expr` / `parse_primary` 宜按族拆文件。
- [x] **LSP format 仍二次严格 `parse_module`**：保持“strict parse 不复用 recovering AST”契约，同时把 `lsp` 格式化缓存从“仅成功 edits”升级为“strict 结果缓存（成功 edits + 失败消息）”；同一 source hash 下重复 formatting 不再重复 strict parse，`didClose` 同步清理 `format_cache`（2026-08-20）。
- [x] **工作区级 clippy allow 仍宽**：已从 4 个 crate 顶（`lumia_ty`/`lumia_core`/`lumia_opt`/`lumia_codegen`）的 12 项 `#![allow(clippy::…)]` 全部下沉到触发模块/函数级；`type_complexity`×2 + `collapsible_match`×1 无触发直接删除；`too_many_arguments` 推至 `lambda_lift/`·`mono/`·`tco`·`dense_f64_sr/`·`escape/`·`closure_cap_tys` 等具体文件；`collapsible_match` 推至 `domain_sr/match_*.rs`·`visit.rs`·`let_bind.rs`；`type_complexity` 推至 `value_ty/mod.rs` 单函数（2026-08-19）。

### 续（2026-08-16 第七轮）
- [x] **`SigShadow` 取代空 `Block` `mem::replace`**：[`signature_shadow`](crates/lumia_core/src/mono/fun_index.rs) / `SigShadow`；`traits` 与 `specialize/rewrite` 原地改活体。
- [x] **RT 全局初始化三轨**：[`globals`](crates/lumia_rt/src/globals.rs) 契约表已补齐并统一登记 `gc/nursery.rs` 的进程级锁自由探测原子（`NURSERY_BASE/END/CURSOR`，`AtomicUsize + Relaxed`），使“新全局只经此表登记”的意图落地。 
- [ ] **编辑器门禁仍半边**：仍欠 IDEA 缩进/注释契约、非 Linux CI、矩阵测。
- [x] **opt/mono 测试密度仍不均**：在既有外置 `mono/tests/` 基础上补齐 `mono/specialize` 的近距回归（`forwarders.rs` 链式 trivial forwarder 折叠、`ret_refresh.rs` clone→generic 精确类型回灌与 `scheme_poly` 保护、`funref.rs` constant-return FunRef/List/Adt 快照），`cargo test -p lumia_core --lib` 全绿（224/224，2026-08-20）。

### 续（2026-08-16 第八轮）
- [ ] **「是否堆」多套启发式未完全合流**：codegen `type_may_heap` 已薄封装 Core；共享 [`builtin_result_may_heap`](crates/lumia_core/src/value_ty/mod.rs) + [`HeapMay`](crates/lumia_core/src/value_ty/mod.rs)（rooting vs slot）。`mono/key::type_is_mono_container` 有意不含 String/Fun。仍欠其余 Builtin / `ret_ty` 完全合流。
- [x] **`emit_value_if` 根状态**：已改为入口 `HashMap` 快照 + 整表 restore（删 `Rc` COW 三路启发式）。

### 续（2026-08-16 第十二轮）
- [x] **`SchedCore: Send` 覆盖 `!Send` Coroutine**：栈在 `home_coro`；扫描指针在 `scan_ptrs`；自然 `Send`。
- [ ] **根分析「未知→非堆」假收口**：[`HeapMay`](crates/lumia_core/src/value_ty/mod.rs) 已文档化双投影（Unknown→root / Unknown↛slot）。真统一仍需显式 `CoreTy::Unknown` lattice。
- [x] **`gc/` 压力原子归属** [`pressure`](crates/lumia_rt/src/gc/pressure.rs)。
- [x] **codegen `emit_value/builtin/` 仍厚**：[`convention.rs`](crates/lumia_codegen/src/emit_value/builtin/convention.rs) 维持表驱动；`show.rs` 已独立 typed show/println 分类；`task.rs` 继续拆为 `task_spawn.rs`（FunRef/closure spawn）与 `task_option.rs`（`recvOpt`/`joinOpt` → Option 组装 + shared root），主文件仅保留 family dispatch 与最薄 custom builtin（2026-08-20）。
- [x] **codegen Show/Eq/Println 特化符号汤已收口**：`ShowForm` + `SPECIALIZED_*_RT` + `call_trait_override`；嵌套 Show/Bool TID/immortal empty/par_map Float→Int 等 BUG 已修（见 git）。
- [x] **`emit_arith` 算术厨房**：已把 `emit_value_binary` 里的整数算术分支收口为统一 helper（`emit_int_add_sub_mul` + `emit_int_div_rem`），压缩了 `Num ICE` / `nsw_iv` 与 `div/rem` checked|unchecked 边界散落（2026-08-20）。
- [x] **`mono/tests` 外置成测沼**：把 `mono/{ret_ty,directize,key}` 旁的近距单测入口迁移到 `crates/lumia_core/src/mono/tests/`（通过 `#[path=...]` + `include!` 复用原测试内容，保持原有 `super::...` 私有可见性与逻辑不变）（2026-08-20）。
- [x] **`fiber.rs`↔`scheduler` 责任边界已抽** [`sched_fiber_api`](crates/lumia_rt/src/task/sched_fiber_api.rs) + `home_coro`。
- [x] **opt `inline.rs` `$inl` 协议**：已废止 `$inl{tag}_`；槽名走 `$s{id}` + 脱糖前缀保留（2026-08-18）。仍欠 IR 级 Local 槽。

## 已落地记录

历史收口与功能完成项详见 git 历史与 [docs/BUILD.md](docs/BUILD.md)。本文件 `[x]` 为经复核已闭环、暂留打叉备查的条目。

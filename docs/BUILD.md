# Lumi 构建与实现计划

> **状态**：最终形态技术栈已落地（骨架可跑）  
> **配套**：语言语义见 [DESIGN.md](DESIGN.md)  
> **最后更新**：2026-08-11

本文档记录 **怎么实现 / 怎么编译**，避免事后忘记选型与约定。语义不妥协版仍以 DESIGN 为准；实现分期可以瘦，**架构不能换**。

---

## 1. 总原则

- **目的地即路线**：Rust 编译器 → Core SSA IR → LLVM → 原生可执行文件 + 可插拔 GC 运行时。
- **必须**：必须编写优雅、统一的代码，健康的项目架构，便于后期维护与发展。
- **本编译器需要支持 Linux、Windows 平台**；在 GitHub（[TetraploidHuman/Lumi](https://github.com/TetraploidHuman/Lumi)）上维护仓库，并以 CI 跑相关测试用例（`cargo test` + examples e2e）。
- **不做**：树遍历解释器、字节码 VM、Cranelift 并行后端、JVM / 自托管编译器作为主路径。
- **允许**：功能子集（少优化 Pass），但 **执行路径始终是 LLVM 产出的机器码**。
- **内存**：回收器可更换；首发分代 mark-sweep（minor STW + 增量并发 full mark），以后可换更强 GC 或 ARC 模式。

```text
Source.lm
  → Parse (lumi_syntax)
  → HIR (lumi_hir)
  → HM + effect infer (lumi_ty)
  → Core SSA (lumi_core)
  → Opt passes (lumi_opt)     §7.1.1：能证则特化，否则默认
  → LLVM codegen (lumi_codegen)
  → link lumi_rt
  → 原生可执行文件
```

一句话：

> **Rust 编译器 + Core SSA + LLVM codegen + 可插拔 GC ABI（首发 mark-sweep）**；优化与表示选择是语言契约；执行物永远是原生代码；**目标平台：Linux 与 Windows**。

---

## 2. 技术栈选型（已钉死）


| 层     | 选择                                                                               |
| ----- | -------------------------------------------------------------------------------- |
| 编译器宿主 | **Rust**（本机 `rustc`/`cargo`，不另装）                                                 |
| LLVM  | **本机 LLVM 21**（inkwell feature `llvm21-1`）；`LLVM_SYS_211_PREFIX` 指向 `llvm-*-dev` |
| 链接    | `clang` + 系统 lld；用户程序再链 `liblumi_rt.a`                                          |
| 解析    | 手写 recursive descent（`lumi_syntax`）                                             |
| 优化 IR | 唯一中端 **Core**（ANF / SSA-ish）                                                     |
| 后端    | **唯一 LLVM**（Debug / Release 都走 LLVM）                                             |
| 泛型    | **单态化**（最终形态）                                                                    |
| 运行时   | `lumi_rt`：Rust，对外 **C ABI**；GC 为可替换模块                                           |
| 首发 GC | 分代 **mark-sweep**：minor STW + remembered-set 写屏障；full 为 **增量并发 mark**（Dijkstra 着色）+ 收尾 remark/sweep；`lumi_write_barrier`；shadow stack 根 |


### 明确拒绝

- 解释器 / 字节码 VM 作为产品执行路径
- 宿主改用 OCaml / C++ / Zig / JVM
- 用户可见 GC 调参、`HashMap`/`TreeMap` 分型、容量 API
- LLVM + Cranelift 双后端并行维护
- 把 retain/release 或某一收集算法写死进 IR（导致无法换 mm）

---

## 3. Cargo workspace

```text
crates/
  lumi          lib+bin：load / check_program / pkg / lsp / doc；CLI 为薄封装
  lumi_syntax   词法 + 递归下降解析，带 Span（AST 在 ast.rs）
  lumi_hir      语法糖降级后的具名 IR + Builtin::info
  lumi_ty       HM 推断 + 效应；共享 typecheck_hir（infer→parallel→effects）
  lumi_core     Core SSA + HIR→Core；pipeline 走 typecheck_hir
  lumi_opt      Pass 管道（§7.1.1）：CSE / Memo / Inline / Escape / ReprSelect / CopyElim
  lumi_codegen  inkwell → .o → clang 链接（Codegen 子状态 + CodegenError）
  lumi_rt       GC ABI + mark-sweep + println*
  lumi_abi      TYPE_*/MEMO_* + float_contract；packing / classifiers 唯一起源
examples/        示例 .lm
scripts/env.sh   NixOS：LLVM_SYS_211_PREFIX + 共享库 PATH（排除 *-static）
scripts/e2e.sh   薄包装 → cargo e2e_examples
scripts/check.sh 本地 CI 冒烟：`cargo test` workspace lib + lumi e2e
```

**abi vs rt（Float / `type_id`）**：`lumi_abi` 拥有 packed `type_id` 构造器、标志位、`ENSURE_*` 符号名与纯分类器；`lumi_rt` 只做指针→header 读取（`list_tid` / `map_tid` / `set_tid`）与 `ensure_*_f64` 语义。C 符号 `lumi_ensure_*_f64` 冻结。

根目录 `[workspace.dependencies]` 已钉 `inkwell` 的 `llvm21-1`。

**前端统一**：CLI、LSP 与 `lumi_core::pipeline` 共用 `lumi_ty::typecheck_hir`（多文件 load / assert 注解仍在 `lumi` lib）。

---

## 4. 本机构建步骤

### 4.1 环境（NixOS / 本机已装 LLVM 21）

```bash
source scripts/env.sh
# 应打印：Lumi env: LLVM_SYS_211_PREFIX=/nix/store/...-llvm-21.1.8-dev
```

要点：

- 必须用 **共享** zlib/libffi/libxml2；**不要**把 `*-zlib-*-static` 放进 `LIBRARY_PATH`（会与 rust-lld 冲突：`incompatible with elf64-x86-64`）。
- `scripts/env.sh` 已过滤 `*-static`。

### 4.2 编译编译器与运行时

```bash
source scripts/env.sh
cargo build -p lumi -p lumi_rt
```

### 4.3 编译用户程序

```bash
cargo run -p lumi -- check examples/hello.lm
cargo run -p lumi -- build examples/hello.lm -o /tmp/hello
/tmp/hello    # → 42

cargo run -p lumi -- build examples/add.lm -o /tmp/add --show-ir
```

`lumi build` 会：`check` → Core → opt → 必要时 `cargo build -p lumi_rt` → LLVM 目标文件 → `clang` 链接 `liblumi_rt.a`。

### 4.4 测试

```bash
source scripts/env.sh
cargo test --workspace
./scripts/e2e.sh
```

---

## 5. GC ABI（稳定合同）

Codegen 与所有 MmBackend 共用；换收集器时优先只改 `lumi_rt` 内实现。


| 符号                                                   | 作用                                |
| ---------------------------------------------------- | --------------------------------- |
| `lumi_alloc(nbytes, type_id) -> *mut u8`            | 堆分配（可触发收集）；返回 **payload** 指针（头在前） |
| `lumi_root_push(*mut *mut u8)` / `lumi_root_pop()` | shadow stack 根                    |
| `lumi_write_barrier(obj, field, new)`               | 写屏障：old 写入 young 时记入 remembered set；young/非堆为 no-op |
| `lumi_gc_collect()`                                 | 强制 **full** 收集（若增量 mark 进行中则先排空） |
| `lumi_println_int` / `_cstr` / `_str` / `_bool`     | 效应 I/O                            |


对象头：`type_id`、`size`、`marked`（见 `crates/lumi_rt`）。容器 `type_id` 为 **base（低 8 位）+ 标志位**：`TID_F_KEY` / `TID_F_VAL` / `TID_ASSOC`（见 `lumi_abi`）；不再为 Float/Assoc 组合单独分配稠密 ID。

**更换难度**：


| 难度  | 场景                                     |
| --- | -------------------------------------- |
| 易   | 同属 tracing：mark-sweep ↔ semispace ↔ 分代 |
| 中   | tracing ↔ ARC（需 codegen 模式开关）          |
| 难   | 无对象头 / 无根约定的裸 malloc 与精确 GC 混用         |


`List`/`Map`/`Set` 更新默认走新分配 / overlay；`List.append` 在 **retain 证明唯一** 且有余量时可原地扩容（COW）。`List.set` 始终新分配，避免 SSA 别名被原地改写。`--mm=arc` 仍可作为另路径。

---

## 6. 优化与表示选择

- Pass 接口在 `lumi_opt`：`cse` / `const_fold` / `licm`（Debug+Release，局部消重）+ Release 的 `memo_tf`（有界 `T_f`：Slots / DenseInt；**CSE 前**做建表规划，非 pass 循环内空跑）；`--no-memo`（别名 `--no-memo-l2`）可关 runtime Memo 做对比。运行时 C 符号仍为 `lumi_memo_l2_*`（ABI 冻结）。
  - **Cargo `opt-*` features（阶段 B）**：`opt-memo` / `opt-dense-f64` / `opt-inline` / `opt-repr-stack`；`lumi` 默认全开。关掉时 schedule 不含对应 Pass，且 codegen/rt 成对剔除声明与 C ABI（`ensure_runtime_built` 按同名 feature 编 `lumi_rt`）。仅 `codegen`、不要优化时：`cargo build -p lumi --no-default-features --features codegen`。
  - **跨 crate capabilities（阶段 C）**：`lumi::CapabilitySet` 统一挂 `hof_fuse`（HIR）/ `auto_parallel`（ty，CLI `--no-parallel`）/ `loop_sr`·`tco`·`nsw_iv`（codegen）；`build`/`check` 经此组装 `LowerOptions` / `TypecheckOptions` / `CodegenOptions`。
  - **统一编译配置（阶段 E）**：`lumi::CompileProfile` = `CapabilitySet` + `PassSet` + build 旋钮；`compile_with_profile` / `check_program_with_profile`；CLI `--no-hof-fuse` / `--no-loop-sr` / `--no-tco` / `--no-nsw-iv`；`lumi build --list-caps` / `--list-passes`。
  - **阶段 F**：CLI pass 开关 `--no-inline` / `--no-dense-f64` / `--no-repr-select` / `--no-escape`；`Lumi.toml` `[compiler]` 与 `.lumi/settings.toml` + 环境变量（`LUMI_NO_PARALLEL` 等）合并进 profile；LSP 经 `CompileProfile::for_lsp_at(path)` 读取工作区配置。
  - **阶段 G**：legacy `compile_with_caps` / `check_program` 标 `deprecated`；`ensure_runtime_built` stamp 按 `lumi_rt` 源码指纹失效；LSP `initializationOptions` / `didChangeConfiguration` 合并进 profile；CI `feature-matrix` 各 `minus-opt-*` leg 跑定向 `cap_regress` / `pass_regress`；`--show-memo-stats` / `LUMI_MEMO_STATS`；`repr_regress`。
  - **阶段 H（部分）**：`--no-memo-dense` / `memo_prefer_dense`；`--mm ms|arc`（Arc：COW + 非 COW 堆对象 `rc→0` 释放；`lumi_heap_retain`/`release`）；`LUMI_GC_MARK_THREADS>1` STW 并行 mark（共享 `HEAP_SET` 视图）。环策略仍靠 mark-sweep。
  - **LSP / 工具前端**：`CompileProfile::for_lsp_at(file)` + `lumi_core::PipelineOptions`；`check_program_with_overlays` / `compile_source_to_core_with_pipeline` 与 CLI 同 caps 语义。
  - **`memo/` 模块 = §7.5 reuse 族**（非单一 pass）：CSE + PE fold + LICM + `T_f` plan/apply；标量环境统一为 `KnownScalars`（与 `SpecializeConst` 共享）。
  - CI `feature-matrix` job 守护 slim / `codegen`-only / 逐个关 `opt-*` 的编译 + `cap_regress` / `pass_regress` / `repr_regress`。
- 测试/工具前端：`lumi_core::PipelineOptions`（`hof_fuse` / `auto_parallel` / `trust_foreign_pure`）或 `CompileProfile::to_pipeline_options()`；多文件加载、visibility、assert 消息注解仍仅 CLI。

**Embedder 示例（`CompileProfile`）**

```rust
use lumi::{CompileProfile, compile_with_profile};
use lumi::compiler_config::{CompilerConfig, CapDisables, PassDisables};
use std::path::Path;

// Stock Release，关掉 inline pass（语义应与 stock 一致，见 cap/pass_regress）。
let profile = CompileProfile::stock(true).without_pass("inline");
compile_with_profile(Path::new("app.lm"), Path::new("app"), &profile)?;

// 或从 manifest + CLI 合并：
let config = lumi::compiler_config::load_for_file(Path::new("app.lm"));
let profile = CompileProfile::assemble(
    true,
    true,  // memo_tf
    false, // trust_foreign_pure
    false, // emit_ir
    vec![],
    &config,
    &CapDisables::default(),
    &PassDisables { no_inline: true, ..Default::default() },
)?;
```

`Lumi.toml` 片段：

```toml
[compiler]
no_parallel = false
no_inline = true
```

环境变量：`LUMI_NO_PARALLEL=1` 等与 `[compiler]` 同义（CLI `--no-*` 优先级最高）。
  - **Inline**：小纯函数直调内联（跳过 `main` / `foreign` / memo / 递归 / 效应）；Release 在 Inline 后再跑 `ConstFold` → `SpecializeConst` → `Escape` → `ReprSelect`（内联露出的字面量可栈分配）。
  - **Escape**：保守逃逸分析；标量/`Join`/字符串深拷贝投影可不 `may_capture`；`Take`/`Elems` 等共享或拷贝元素指针的仍捕获；逃逸的 `ListGet`/`AdtField` 会标容器。`ReprSelect` 对**未逃逸**小 `List`/`Map` 标 `LitList` / `SmallMap`（codegen 栈布局已接）。
  - **SpecializeConst**：Int/Bool/Char 调用点常量特化（`f$c_…`）；Release 在 Inline 前后各一轮。
  - **CopyElim**：折叠 `let x = y` SSA 别名。
  - **concat_ident**：Core 消 `concat([])` 恒等（`map`/`filter`/`fold` 主融合在 HIR）；空 `listOf()` → `lumi_list_empty` 永生单例。
  - **稳健性**：foreign `String` 临时 cstr 在调用期间入根（防 GC UAF）；Iota 物化 / 取下标用 checked 算术并对过大物化 trap；跨 product 同名字段的 `with` 报歧义。
  - **List Iota**：`range` / `rangeInclusive` → `TYPE_LIST_IOTA`（`[start,end)`，O(1)）；`len`/`get`/eq/hash/`take`/`slice` 虚拟；修改类 API `force` 成 HeapList（见 `examples/range_iota.lm`）；PE 跟踪虚拟 iota；`par_map`/`concat` 空恒等不强制物化。
- GC：分代 mark-sweep（young 默认 1MiB → minor STW：只标记 nursery + remembered/rooted old；old 默认 8MiB → **增量并发 full mark**，或 `lumi_gc_collect` 排空）+ **`lumi_write_barrier`**（remembered set + Dijkstra 着色）+ **shadow-stack 根**；`is_heap_payload` O(1)；见 `examples/gc_roots.lm`。
  - **Escape**：短生命周期 `var` 不再一律逃逸；经 `Name`/返回逃逸的赋值仍会标记，便于 `ReprSelect` 选栈 `Lit*`。
- Map：小表线性 Assoc；超过 8 对晋升 **HashOrdered**；大表 `set` 走 **Overlay** 差分（满 8 条再压实）；见 `examples/map_hash.lm`。
- Set：同哲学 — ≤8 线性，更大 **HashOrdered**（开址 + 插入序）；见 `examples/set_hash.lm`。
- 元组投影：`p.0` / `p.1`（`examples/tuple_fields.lm`）；Fun 效应变量 + HOF 拾取 IO（`examples/effect_hof.lm`）。
- 集合字面量糖：`[:]` / `[k : v]` → `mapOf`；`#{}` / `#{a,b}` → `setOf`（`examples/coll_lit.lm`）。
- `sortBy`：键为 `Int` / `String` / `Char`（稳定）；`assert(cond)` 失败即中止，并打印 `path:line`（`examples/assert_ok.lm`）。
- 诊断：`path:line:col: kind: message` + 源码行与 `^`（parse / lower / type）；**多文件**按 `Span.file` 归到正确源文件（`examples/bad_import_type.lm`）。
- `for (k, v) in m` 自动 `.items()`；若迭代器已是 pair 列表（`m.items()` / `….sortBy(…)`）则不再套一层（WordCount）。
- `lumi fmt`：基础 pretty-print（4 空格）；`--check` 只校验。
- 旗舰示例：`examples/word_count.lm`（DESIGN §14：stdin → 分词计数 → `items().sortBy` 打印）。
- **包管理**：`Lumi.toml` path 依赖 + `lumi pkg init|lock|add`；有 deps 时**必须**有 `Lumi.lock`；`package.link` 自动并入链接参数（见 `examples/use_path_dep.lm`）。
- **LSP**：`lumi lsp`（stdio；未保存 buffer overlay；诊断；hover；跨文件定义；补全；formatting）。
- **FFI**：`foreign "C" [pure] fn …`（`Int`/`Bool`/`Float`/`Unit`/`String↔cstr`）+ `--link` / `package.link`（`examples/ffi_abs.lm` / `ffi_strlen.lm` / `ffi_getenv.lm`）。默认效应为 IO；`pure` 需 `--trust-foreign-pure` 或 `package.trust_foreign_pure = true`（荣誉系统，未验证）。
- **自动并行**（默认开）：无捕获 lambda 或顶层函数名的纯标量 `List.map` → `ListParMap`（`examples/par_map.lm` / `par_map_fn.lm`）；IO/堆类型/捕获闭包回退顺序（`par_map_capture.lm` / `bad_par_map_io.lm`）。`--no-parallel` 关闭。worker 内禁止堆分配（TLS 堆隔离）。
- Memo 性能：`scripts/bench_memo.sh`（同参热命中，约 **20×** vs `--no-memo`；报时间 + 峰值 RSS）；`examples/memo_dense.lm` 的 `fib` 下标表约 **1000×+**。
  - `**bench_cpu` 整套**：收益几乎只来自 `fib`（其余核是单遍扫参，无跨调用复用 → 理论无命中）。曾有成本模型把「循环里调用一次」当成命中证据、误挂 4 槽表导致 Collatz **变慢**，已改为要求递归或静态同参复用；稠密表仅结构递减自递归。
- CPU 计算密集：`scripts/bench_cpu.sh`（素数 / matmul / Mandelbrot / Collatz dense+strided / fib / poly / gcd / divisorSum / productRem / floatOrbit / rangeFold；约 0.5–1s 量级，报 min/median/max **时间 + 峰值 RSS**）。
- Dense float（热路径）：`scripts/bench_cn_hot.sh`（naive 循环 vs `std.linalg`；checksum 对齐 + 时间/RSS）。
- Dense float（整步）：`scripts/bench_cn_step.sh`（sensory fill/scale/add + gate mul + decay + PC/Hebbian；扩展 SR 面）。
- EFE action scores：`scripts/bench_cn_efe.sh`（imagine+G(a) naive vs fused `lumi_efe_action_scores`）。
- **表示 / COW 专项**：`scripts/bench_repr.sh`（共享 `take` / Iota / `drop` consume / take+append / 唯一 `concat` / 唯一 `reverse` / 只读 `Map`；checksum + 时间/RSS）。
- **聚合回归**：`scripts/bench_all.sh` 依次跑 cpu / memo / repr / cn_hot / cn_step / cn_efe（改 dense-float 等优化时应用此入口，避免单项过关、旧核回归）。
- **峰值 RSS**：`scripts/bench_measure.sh` 经小型 C 父进程 `wait4`（`scripts/peak_rss.c`）取样；勿用大 RSS 的 Python `subprocess` fork——COW 会把解释器常驻内存算进子进程 `ru_maxrss`。Release 链接加 `--gc-sections`（macOS：`-dead_strip`）丢掉未引用的 `lumi_rt`/Rust-std 目标文件，降低基线 RSS。
- **纪律（DESIGN §7.1.1）**：分析能证明 → 特化；不能证明 → **默认稳定路径**：
  - `List` → `HeapList` / `COWList`
  - `Map`/`Set` → `HashOrdered` + COW / Overlay
- Debug：少融合 / 少特化，仍走 LLVM；语义与 Release 一致。

---

## 7. 分期交付（架构不变）


| 阶段              | 内容                                                                                                                                            |
| --------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| **已完成骨架**       | parse 子集 → 推断 + 效应 → Core → LLVM → 链 `lumi_rt` → `main` + `println` + `Int`；`listOf`→`AllocList`；CSE + ReprSelect 默认路径                       |
| **已完成下一步（部分）**  | …；**sortBy / assert+行号**；**定位诊断（多文件）**；**Map Overlay**；**WordCount**；**lumi fmt**；…                                                          |
| **已完成（相对原「下一里程碑」）** | Trait / instance + 运行时字典；非逃逸小对象栈分配（Lit* / LitAdt + 晋升）；`std.option` / `std.result` / `std.string` / `std.io` 源文件正文；逃逸分析 / 融合 / TCO SCC / 自动并行 / 透明 Memo；local `Map.get` PE (§7.5.1-A) + Release 二次 `const_fold`；**Int/Bool/Char call-site specialization**（`SpecializeConstPass`）+ 字面 `ListTake`/`ListSlice`/`ListReverse`/`AdtTag`/`Map.set`/`Set.insert` PE |
| **仍待** | 更完整的 Arc 环检测/破环策略；进程级长期共享堆（测试仍用 TLS 隔离） |
| **工具链已落地** | **自动并行**（默认 `ListParMap` + 不安全回退；`--no-parallel`）；**包管理**（`Lumi.toml` / `lumi pkg`）；**LSP**（`lumi lsp`）；**FFI**（`foreign "C" fn`）；`priv` 跨文件可见性；`effect { }` 块；Map/Set `finish` 晋升；`lumi fmt` / `lumi doc` |


每一阶段用户看到的都是 `**lumi build` 产出的原生程序**。

### 示例

```bash
cargo run -p lumi -- build examples/match.lm -o /tmp/m && /tmp/m   # 20
cargo run -p lumi -- build examples/for.lm -o /tmp/f && /tmp/f     # 15\\n3
cargo run -p lumi -- build examples/list_for.lm -o /tmp/l && /tmp/l # 60
cargo run -p lumi -- build examples/break.lm -o /tmp/b && /tmp/b   # 4
cargo run -p lumi -- build examples/list_match.lm -o /tmp/lm && /tmp/lm  # 0\\n7
cargo run -p lumi -- build examples/to_map.lm -o /tmp/tm && /tmp/tm # 2
cargo run -p lumi -- build examples/map_ops.lm -o /tmp/mo && /tmp/mo
cargo run -p lumi -- build examples/option_match.lm -o /tmp/om && /tmp/om  # 0\\n7
cargo run -p lumi -- build examples/point.lm -o /tmp/pt && /tmp/pt  # 3\\n4\\n10\\n4\\n3
cargo run -p lumi -- build examples/use_math.lm -o /tmp/um && /tmp/um  # 42\\n42
cargo run -p lumi -- build examples/use_priv.lm -o /tmp/up && /tmp/up  # 42\\n42
cargo run -p lumi -- build examples/use_pkg.lm -o /tmp/upkg && /tmp/upkg  # 42\\n42
cargo run -p lumi -- build examples/list_hof.lm -o /tmp/hof && /tmp/hof  # 5\\n2\\n3\\n24
cargo run -p lumi -- build examples/list_hof_fn.lm -o /tmp/lhof && /tmp/lhof  # 10\\n30\\n1\\n3\\n6
cargo run -p lumi -- build examples/list_concat.lm -o /tmp/lc && /tmp/lc  # 5\\n1\\n5\\n30
cargo run -p lumi -- build examples/list_pipe.lm -o /tmp/lp && /tmp/lp  # 3\\n6\\n10
cargo run -p lumi -- build examples/list_set.lm -o /tmp/ls && /tmp/ls  # 1\\n99\\n3\\n2\\n3
cargo run -p lumi -- build examples/match_guard.lm -o /tmp/mg && /tmp/mg  # 1\\n2\\n0
cargo run -p lumi -- build examples/match_cond.lm -o /tmp/mc && /tmp/mc  # 1\\n0\\n-1
cargo run -p lumi -- build examples/logic.lm -o /tmp/lg && /tmp/lg  # 1\\n10
cargo run -p lumi -- build examples/string_ops.lm -o /tmp/so && /tmp/so  # 5\\nhello\\n2
cargo run -p lumi -- build examples/string_interp.lm -o /tmp/si && /tmp/si  # hello Lumi\\nn=42\\n43\\nplain\\ndollar=$n
cargo run -p lumi -- build examples/string_eq.lm -o /tmp/se && /tmp/se  # 1\\n1\\n1\\n1.5
cargo run -p lumi -- build examples/fib.lm -o /tmp/fib && /tmp/fib  # 55
cargo run -p lumi -- build examples/char.lm -o /tmp/ch && /tmp/ch  # A\\n1\\n1\\nZ
cargo run -p lumi -- build examples/float_ops.lm -o /tmp/fo && /tmp/fo  # 3.75\\n6\\n1\\n-1.5\\n4
cargo run -p lumi -- build examples/closure.lm -o /tmp/cl && /tmp/cl  # 42\\n11
cargo run -p lumi -- build examples/closure_capture.lm -o /tmp/cc && /tmp/cc  # 42\\n101\\n42
cargo run -p lumi -- build examples/range_fold.lm -o /tmp/rf && /tmp/rf  # 499999500000\\n5050
cargo run -p lumi -- build examples/range_map.lm -o /tmp/rm && /tmp/rm  # 5\\n2\\n10\\n5\\n1\\n9\\n249999500000
cargo run -p lumi -- build examples/set_ops.lm -o /tmp/so2 && /tmp/so2  # 3\\n1\\n0\\n3\\n2\\n0\\n1\\n3\\n1
cargo run -p lumi -- build examples/set_algebra.lm -o /tmp/sa && /tmp/sa
cargo run -p lumi -- build examples/coll_conv.lm -o /tmp/cc2 && /tmp/cc2
cargo run -p lumi -- build examples/for_map_set.lm -o /tmp/fms && /tmp/fms  # 6\\n3\\n30
cargo run -p lumi -- build examples/fuse_hof.lm -o /tmp/fh && /tmp/fh  # 24\\n250500
cargo run -p lumi -- build examples/result_match.lm -o /tmp/rmatch && /tmp/rmatch  # 5\\n-1\\n3
cargo run -p lumi -- build examples/list_extras.lm -o /tmp/lex && /tmp/lex
cargo run -p lumi -- build examples/prelude_option.lm -o /tmp/po && /tmp/po  # 10\\n-1\\n42\\n7
cargo run -p lumi -- build examples/string_more.lm -o /tmp/sm && /tmp/sm
cargo run -p lumi -- build examples/map_string_keys.lm -o /tmp/msk && /tmp/msk
printf '  hi hi there  ' | $(cargo run -q -p lumi -- build examples/read_stdin.lm -o /tmp/rs >/dev/null && echo /tmp/rs)
printf 'Hello World\nhello there\nWORLD\n' | $(cargo run -q -p lumi -- build examples/word_count.lm -o /tmp/wc >/dev/null && echo /tmp/wc)
cargo run -p lumi -- build examples/list_text.lm -o /tmp/lt && /tmp/lt
cargo run -p lumi -- build --release examples/memo_tf.lm -o /tmp/memo && /tmp/memo
cargo run -p lumi -- build examples/memo_local.lm -o /tmp/memo_local && /tmp/memo_local
# Memo `T_f` microbench (with vs without cache):
#   ./scripts/bench_memo.sh
# CPU compute suite (primes / matmul / Mandelbrot / Collatz / fib):
#   ./scripts/bench_cpu.sh
#   COMPARE_DEBUG=1 ./scripts/bench_cpu.sh   # also time Debug vs Release
# Dense-float CN hot path + full perf gate (time + peak RSS):
#   ./scripts/bench_cn_hot.sh
#   ./scripts/bench_all.sh
cargo run -p lumi -- build examples/mapset.lm -o /tmp/ms && /tmp/ms
```

---

## 8. CLI 约定


| 命令                                                                                                         | 职责                                                                |
| ---------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| `lumi check <file> [--no-parallel] [--no-hof-fuse] [--no-loop-sr] [--no-tco] [--no-nsw-iv] [--list-caps] [--trust-foreign-pure]` | 解析 + 类型 / 效应 |
| `lumi build <file> [-o out] [--release] [--no-memo] [--no-memo-dense] [--show-memo-stats] [--show-gc-stats] [--mm ms\|arc] [--no-parallel] [--no-hof-fuse] [--no-loop-sr] [--no-tco] [--no-nsw-iv] [--no-inline] [--no-dense-f64] [--no-repr-select] [--no-escape] [--list-caps] [--list-passes] [--trust-foreign-pure] [--link ARG]… [--show-ir] [--emit-llvm]` | 原生二进制；cap / pass / memo / mm / stats 开关见上；`--list-caps` / `--list-passes` 列 inventory 后退出 |
| `lumi fmt [files…] [--check]`                                                                             | 基础 pretty-print（4 空格）；`--check` 不写回                               |
| `lumi doc <file> [-o out.md]`                                                                             | 从 `///` 与公开 API 生成 Markdown（DESIGN §13）                            |
| `lumi lsp`                                                                                                | LSP（overlay 诊断 + hover + 跨文件定义 + 补全 + format）                     |
| `lumi pkg init` / `lumi pkg lock` / `lumi pkg add`                                                      | `Lumi.toml` / `Lumi.lock`；有 deps 时构建要求 lock；`package.link` 并入链接 |


包管理：`Lumi.toml` + lockfile 由 `lumi pkg` 管理；**不**把 Cargo 暴露给用户程序。

**`--link` 信任模型**：CLI `--link` 允许绝对 `-L` / `.a`（本机显式意图）。`package.link` 路径限制在包根下。对不可信源码树，任意链接参数等同原生 RCE 面——不要对不可信输入开启宽 `--link`；沙箱需在宿主层做。

**`readStdin` 软上限**：`lumi_rt` 在约 64MiB 后 `trap_abort`（防恶意/巨型 stdin 拖垮主机）。流式读取或可恢复错误需语言层 `Result`/分块 API，当前为故意硬失败。

---

## 8.1 平台与 CI

- **目标 OS**：Linux、Windows（x86_64）；macOS 非当前必达。
- **仓库**：[https://github.com/TetraploidHuman/Lumi](https://github.com/TetraploidHuman/Lumi)
- **CI**：GitHub Actions（`.github/workflows/ci.yml`）在 `ubuntu-latest` 与 `windows-latest` 上：
  1. 安装 LLVM 21 开发前缀 + `clang`（Linux：`install-llvm-action`；Windows：`vovkos/llvm-package-windows` 完整 SDK，因官方 Windows 安装包不含 `llvm-config`/C++ libs）
  2. 设置 `LLVM_SYS_211_PREFIX`（路径不含空格）
  3. `cargo test --workspace --exclude lumi` 与 `cargo test -p lumi --tests`（含 e2e examples）
- 本地 Linux：`source scripts/env.sh && ./scripts/check.sh`（或 `./scripts/e2e.sh`）
- 本地亦可：`cargo test -p lumi --test e2e_examples`

---

## 9. 常见问题

**链接报 `libz.a ... incompatible with elf64-x86-64`**  
`LIBRARY_PATH` 里混入了 Nix 的 **static** zlib。重新 `source scripts/env.sh`，确认路径中无 `*-static`*。

**找不到 `llvm-config` / inkwell 编不过**  
设置或检查 `LLVM_SYS_211_PREFIX` 指向带 `bin/llvm-config` 的 `llvm-21.1.8-dev` store 路径。

`**liblumi_rt.a` not found**  
先 `cargo build -p lumi_rt`；或直接 `lumi build`（会自动构建 runtime）。

---

## 10. 相关文件


| 路径                                   | 说明                          |
| ------------------------------------ | --------------------------- |
| [DESIGN.md](DESIGN.md)               | 语言设计（语义合同）                  |
| `scripts/env.sh`                     | 本机构建环境                      |
| `scripts/e2e.sh`                     | 薄包装：`cargo build` + `cargo test -p lumi --test e2e_examples` |
| `crates/lumi/tests/e2e_examples/`   | 跨平台 examples e2e（主路径）        |
| `.github/workflows/ci.yml`           | Linux / Windows CI          |
| `crates/lumi_rt/src/lib.rs`         | GC ABI + mark-sweep         |
| `crates/lumi_opt/src/lib.rs`        | Pass 管道                     |
| `Cargo.toml`                         | workspace + inkwell LLVM 21 |



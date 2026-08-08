# Lumia — 本轮未完成 / 待推进

记录经查实、本轮无法完整修复或需更大设计落地的问题。语义以 [docs/DESIGN.md](docs/DESIGN.md) 为准；分期见 [docs/BUILD.md](docs/BUILD.md)。

## 类型与单态化

- [ ] **单态化管线**：let-polymorphism 已落地，但 codegen 仍共享一份闭包/函数体。`id` 同时用于 `Float` 与 `Int` 时，`println(id(1.5))` 可能按 Int 打印 bit pattern。最终形态需按 BUILD「单态化」特化。
- [ ] **类型类 `trait` / `instance` / `requires`**：语法保留并拒绝；Eq/Ord/Hash/Show 派生与 DESIGN §3.6 尚未实现。
- [ ] **`import … as` / `{ name as alias }`**：DESIGN §9.3 已写，解析器仍报 not supported。

## 语义与运行时

- [ ] **Float 作 Map/Set 键与 `lumia_eq`**：`lumia_eq` 以 bit 相等短路；相同 NaN 位在 `contains` 中可命中，与 DESIGN §2.1（`NaN ≠ NaN`）及直接 `==`（IEEE OEQ）不一致。需装箱 Float 或按类型分派 eq。
- [ ] **`foreign "C" pure` 荣誉系统**：效应检查信任注解；撒谎的 `pure` 可把 C 副作用带进纯上下文。opts 已隔离 external，但效应边界仍信任标注。需 `unsafe`/显式信任开关或默认 IO。
- [ ] **stdin 超大输入**：`lumia_rt` 在约 64MiB 后 `abort`，非可恢复错误。
- [ ] **CLI `--link`**：绝对 `-L`/`.a` 故意放行（本机链接）；对不可信树等同原生 RCE 面，文档/沙箱策略待定。

## 优化与表示（DESIGN / BUILD 下一里程碑）

- [ ] **纯互递归 TCO**（DESIGN §4.4）：无 `musttail` / tailcc。
- [ ] **自动并行**：设计为对用户透明；实现为 opt-in `--parallel`，且有捕获/标量限制。
- [ ] **逃逸分析 → 栈分配 / 多表示 List·Map·Set**：大量仍为 HeapList；BUILD §7 下一里程碑。
- [ ] **部分求值 / 完整 specialization**：opt 管道未齐。
- [ ] **`std/` 可执行正文**：现为 `@exports` stub，体在编译器 builtins。

## 工具链

- [ ] **`lumia doc`**：DESIGN §13，无 CLI 子命令。
- [ ] **并发 GC / `--mm=arc`**：BUILD 远期；写屏障在 STW 下为空（正确，非缺口）。
- [ ] **Bool 模式 `b match { true -> … }`**：解析失败；需接受 Bool 模式或给出明确诊断（引导用 `if` / 无主语 match）。

## 本轮已修（便于对照）

- Ord：`<` 等走 `lumia_cmp`；拒绝非 Ord 类型；String 字典序。
- let-polymorphism（HM scheme）。
- const-fold 比较结果保持 `Bool`。
- `List[Float]` 算术按 `local_tys` 走 IEEE。
- LICM/CSE 不再提升/合并可 trap 的算术与 Range/AdtField。
- 词法：非法字节、未闭合字符字面量为 Error。
- LSP Windows `file:///C:/…` URI。
- release 链接落到 debug `lumia_rt` 时告警。

# Lumia — 本轮未完成 / 待推进

记录经查实、本轮无法完整修复或需更大设计落地的问题。语义以 [docs/DESIGN.md](docs/DESIGN.md) 为准；分期见 [docs/BUILD.md](docs/BUILD.md)。

## 类型与单态化

- [ ] **单态化管线**：按 call-site 实参 ground 键克隆：`$Float`/`$Bool`/`$String`/`$List_*`/`$Map_*`/`$Set_*`/`$Option_*`（`poly_*` / `poly_map_id` / `poly_set_id` / `poly_unwrap`）；**FunRef HOF** 多轮克隆 + 体内直连（`poly_option_map` / `poly_option_and_then` / `poly_result_map`）；同名多体共存；`instance Num` 已接线。scheme 驱动仍待。
- [ ] **类型类 `trait` / `instance` / `requires`**：显式 instance 可填 trait 默认方法；Show/Eq/Ord/Num 覆盖已接线；UFCS `x.show()` / `x.eq` / `x.less` 及**任意用户方法**经 `trait_methods` 表解析为 `__Trait_Type_method`（`trait_custom_method` / `trait_custom_default`）；积/和自动派生 Eq/Show；**Hash opt-in**；**多态方法**经约束 + 单态后解析（`trait_poly_show` / `trait_poly_method`）。运行时字典仍待。
- [x] **`import … as` / `{ name as alias }`**：DESIGN §9.3；`ImportedName` + 公开项改名、原名 `priv` 副本；e2e `import_as` / `bad_import_as_original`。

## 语义与运行时

- [x] **Float 作 Map/Set 键与 `lumia_eq`**：`TYPE_MAP_F64` / `TYPE_SET_F64` + IEEE `float_key_eq`/`float_key_hash`（±0 碰撞、NaN 永不命中）；codegen 在 Float 键/`set` 时 `lumia_ensure_*_f64`；e2e `float_map_keys`。
- [x] **Float 结构相等（List / ADT / Map 值）**：`TYPE_LIST_F64`、`TYPE_MAP_VF64` / `TYPE_MAP_F64V`；ADT Float 字段走 typed `fcmp OEQ`；e2e `float_struct_eq`。`ListParMap` 传 `result_tid`（恒等 FunRef + `List[Float]` 保 IEEE 标签）。
- [ ] **标量路径 `lumia_eq` 未装箱 Float**：非堆 i64 仍 bit 短路；标量 `==` 走 codegen `fcmp`，通常不经过此路径。嵌套「无类型标签的 Float 位」仍可能漏网（依赖容器 type_id / typed eq）。
- [ ] **`AssocList` Map + Float 值**：无 Hash 键仍为 `TYPE_MAP_ASSOC`，值侧 IEEE 标签未接线（与 VF64 组合待做）。
- [ ] **陷阱栈追踪**（DESIGN §2 / 错误表）：`trap_abort` 仅打印消息后 abort，尚无用户可见 backtrace。
- [x] **`foreign "C" pure` 荣誉系统**：默认 foreign 为 IO；`pure` 需 `--trust-foreign-pure` 或 `package.trust_foreign_pure`；opts 仍不 CSE/memo external。
- [x] **stdin 超大输入**：约 64MiB 软上限经 `trap_abort`（BUILD §8）；流式/`Result` 另项。
- [x] **CLI `--link`**：BUILD §8 写明信任模型（CLI 绝对路径 = 本机意图；不可信树需宿主沙箱）。
- [x] **`extern "C"` 其余 `panic!`**：已统一经 `trap_abort`（非 test 构建 abort；`cfg(test)` 下仍 panic 以便单测）。

## 优化与表示（DESIGN / BUILD 下一里程碑）

- [ ] **纯互递归 TCO**（DESIGN §4.4）：纯 SCC `musttail`（标量 + 堆参 List/String/ADT；`tco_sum` / `tco_list_sum` / …）；musttail 前 `root_pop_to(0)`，callee 入口再 root；**含 IO 的 SCC 亦可 musttail**（`tco_io_countdown`；DESIGN 不保证）。未知闭包 IndirectCall 未做。
- [x] **自动并行**：默认对 FunRef-safe 纯标量 `List.map` / `List.fold` 选 `ListParMap` / `ListParFold`（`par_map*` / `par_fold`）；顶层-only 自由变量的 lambda 亦安全；IO/非标量/真捕获回退；`--no-parallel` 关闭。`fold` 假定结合律。
- [ ] **逃逸分析 → 栈分配 / 多表示 List·Map·Set**：未逃逸且 ≤8 的 `listOf`/`mapOf`/`setOf` → `LitList`/`LitMap`/`LitSet`（`small_*`）；**已知纯 callee 形参摘要**不误伤（`escape_pure_len`）；**非逃逸 product → LitAdt**（`small_adt_local`）。更多表示 / 晋升仍待。
- [ ] **部分求值 / 完整 specialization**：L0 折叠字面 `ListLen`/`ListGet`/`AdtField`（`pe_list_len_get` / `pe_adt_field`）；**Map/Set `len`/`contains`**（`pe_map_contains`）。完整 specialization 仍待。
- [ ] **`std/` 可执行正文**：`std.option` / `std.result` 已为源文件正文并经 loader 内联（`std_option` / `std_result`）；`std.io` / `std.string` 仍为 `@exports` + builtins。

## 工具链

- [x] **`lumia doc`**：CLI 生成 Markdown（`///`、公开 `val`/`type`/`foreign`、`@exports`）；`priv` 默认隐藏。
- [ ] **并发 GC / `--mm=arc`**：BUILD 远期；写屏障在 STW 下为空（正确，非缺口）。

## 本轮已修（便于对照）

- Ord：`<` 等走 `lumia_cmp`；拒绝非 Ord 类型；String 字典序。
- let-polymorphism（HM scheme）。
- const-fold 比较结果保持 `Bool`。
- `List[Float]` 算术按 `local_tys` 走 IEEE。
- LICM/CSE 不再提升/合并可 trap 的算术与 Range/AdtField。
- 词法：非法字节、未闭合字符字面量为 Error。
- LSP Windows `file:///C:/…` URI。
- release 链接落到 debug `lumia_rt` 时告警。
- **`lumia_trap_*` / `match_fail` / NUL cstr**：`extern "C"` 边界用 abort，避免 panic unwind 二次崩溃。
- **空 match / 仅有守卫臂**：穷尽性检查拒绝（不再误放行）。
- **运行时 `trap_abort`**：致命错误统一入口，避免跨 `extern "C"` unwind。
- **常量模式**：`true`/`false`、`Char`、`String`、`Float`、负数字面量（含 `-1` / `-1.5`）；Bool 双臂穷尽。
- **poly identity Float**：call-site 对堆哨兵 `ret_ty` + Float 实参恢复 Float，避免 `println(id(1.5))` 打印 bit pattern。
- **`import … as`**：模块别名导入与原名不可见。
- **Float Map/Set 键**：IEEE eq/hash 专用 type_id；与 `==` 对齐的 ±0 / NaN 行为。
- **`foreign "C" pure`**：默认 IO；`pure` 需显式信任开关。
- **`lumia doc`**：Markdown API 文档生成。
- **自动并行默认开**：推断后保留/回退 `ListParMap`；`--no-parallel` 关闭。
- **多态 trait 方法**：`{ x -> x.show() }` / `{ x -> x.toInt() }` 多实例单态；缺 instance 拒绝。
- **Opt**：Map/Set PE；逃逸 callee 摘要；LitAdt；IO SCC musttail。
- **Mono Map/Set + HOF**：`$Map_*`/`$Set_*`；Option/Result `optMap`/`andThen`/`resultMap` FunRef 多轮单态。
- **`std.option` / `std.result`**：源文件组合子 + loader `StdKind::Source` 内联。

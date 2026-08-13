# Lumia — 本轮未完成 / 待推进

记录经查实、本轮无法完整修复或需更大设计落地的问题。语义以 [docs/DESIGN.md](docs/DESIGN.md) 为准；分期见 [docs/BUILD.md](docs/BUILD.md)。

## 类型与单态化

- [x] **产品字段类型导向解析**：歧义 `.f` → `AdtField(_, -1, field)`；`with` 在字段集无法唯一确定 product 时保留 HIR `With`；ty 按接收者/`with` 基表达式的具体产品消解并 rewrite（开放接收者仍报错）。e2e `shared_product_field`。
- [x] **`with` 开放接收者类型**：`lower_with` 未拷贝字段时曾发 2 参 `AdtField`，开放 `p` 被收成 `TuplePrefix`（`{ p -> p with { x = … } }` 无法返回具名产品）。已改为与 `.field` 相同的 3 参（带 product 名）。
- [x] **单态化管线**：按 call-site 实参 ground 键克隆：`$Float`/`$Bool`/`$String`/`$List_*`/`$Map_*`/`$Set_*`/`$Option_*`（`poly_*` / `poly_map_id` / `poly_set_id` / `poly_unwrap`）；**FunRef HOF** 多轮克隆 + 体内直连（`poly_option_map` / `poly_option_and_then` / `poly_result_map`）；同名多体共存；`instance Num` 已接线。**scheme 驱动**：`TypedModule.fun_schemes` + `CoreFun.scheme_poly`；仅 `needs_mono` / 抬升 lambda / FunRef HOF 克隆。
- [x] **类型类 `trait` / `instance` / `requires`**：显式 instance / UFCS / 派生 / Hash opt-in / 多态方法经单态解析已接线；**运行时字典** `lumia_dict_register/lookup`（Show/Eq/Ord/Hash/Num）在 `main` 启动注册 mangled instance（动态分发可查表；热路径仍直连单态）。
- [x] **`import … as` / `{ name as alias }`**：DESIGN §9.3；`ImportedName` + 公开项改名、原名 `priv` 副本；e2e `import_as` / `bad_import_as_original`。
- [x] **开放产品字段投影**：`{ p -> p.x }` 将接收者约束为具名产品（拒绝 `getx(1)`）；`PRODUCTS` 在 lower 后保留供 ty 查询。
- [x] **开放位置投影 `.0`**：`Type::TuplePrefix`「至少 N」约束；`{ t -> t.0 }` 接受更长元组、拒绝 `Int`（`open_tuple_proj_*` / `bad_tuple_proj`）。
- [x] **`val` 解构绑定**：块内 `val (a, b) = p` / 嵌套元组 / `val Point { x, y } = p`（不可反驳）；可反驳模式拒绝并提示用 `match`；e2e `let_destructure` / `bad_let_destructure`。
- [x] **`for (k, v) in`**：Map→`items`；已是 pair 列表则直拆；e2e `for_pair_list` / `for_map_set` / `for_destructure`。
- [x] **多态 UFCS 采样**：对开放接收者不再把 call 与某个 sample instance 全量 unify（避免冻成首个 instance 类型）。

## 语义与运行时

- [x] **有限 `return` + `alt`**：最近函数/闭包早退；`expr alt rhs` 恢复 Option/Result（Result 绑定 `err`）；传播写 `alt return Err(err)`（无自动包装、无裸 `alt return`、`?` 仍搁置）。
- [x] **Float 作 Map/Set 键与 `lumia_eq`**：`TYPE_MAP_F64` / `TYPE_SET_F64` + IEEE `float_key_eq`/`float_key_hash`（±0 碰撞、NaN 永不命中）；codegen 在 Float 键/`set` 时 `lumia_ensure_*_f64`；e2e `float_map_keys`。
- [x] **Float 结构相等（List / ADT / Map 值）**：`TYPE_LIST_F64`、`TYPE_MAP_VF64` / `TYPE_MAP_F64V`；ADT 经 `lumia_adt_eq(float_mask)`（按**对象实际 size**，非 type-param 元数）；e2e `float_struct_eq` / `adt_float_eq`。
- [x] **嵌套 Float ADT 的 eq/hash**：ADT header `_pad` 存 per-field Float mask；`lumia_eq` / `hash_value` / `lumia_adt_eq` 读 mask（IEEE）；codegen `lumia_adt_set_float_mask`；e2e `nested_float_adt_eq`。
- [x] **ADT 字段 GC**：`mark` 对 `TYPE_ADT` 按 `_pad` 跳过 Float 槽。
- [x] **`lumia_show` 集合格式**：List/Map/Set 递归展示元素（`[…]` / `{k: v}` / `#{…}`）；积/和 ADT 仍为 `#tag(…)`。
- [x] **标量路径 `lumia_eq` 未装箱 Float（契约锁定）**：非堆 i64 仍 bit 短路（`lumia_rt`/`lumia_abi::float_contract` 单测）；标量 `==` 走 codegen `fcmp`；IEEE ±0/NaN 仅容器 typed eq / `fcmp`。嵌套无标签 Float 位不在 `lumia_eq` 保证范围。
- [x] **`AssocList` Map + Float 键/值**：`TYPE_MAP_ASSOC_{F64,VF64,F64V}`；空 ASSOC 可 `ensure_*` 转标签且永不 Hash 晋升；codegen `mapOf` 在无 Hash 时选用 ASSOC_* 标签。
- [x] **陷阱栈追踪**（DESIGN §2）：codegen `lumia_frame_push/pop` + `trap_abort` 打印 Lumia 调用栈；musttail 前 pop。
- [x] **纯函数内嵌 lambda 效应**：构造 IO 闭包为纯（Fun 携 ε）；`assert_no_effects_in_pure` 进入 lambda 体作效应上下文；跟踪 let 绑定 IO thunk；e2e `pure_io_thunk`。
- [x] **`println` 默认 `Var→Int`**：已取消冻成 Int；开放 Var 走 `println_auto`；允许 List/Map/Set/Tuple。
- [x] **`foreign "C" pure` 荣誉系统**：默认 foreign 为 IO；`pure` 需 `--trust-foreign-pure` 或 `package.trust_foreign_pure`；opts 仍不 CSE/memo external。
- [x] **stdin 超大输入**：约 64MiB 软上限经 `trap_abort`（BUILD §8）；读错误亦 `trap_abort`（不再当 EOF）；流式/`Result` 另项。
- [x] **CLI `--link`**：BUILD §8 写明信任模型（CLI 绝对路径 = 本机意图；不可信树需宿主沙箱）。
- [x] **`extern "C"` 其余 `panic!`**：已统一经 `trap_abort`（非 test 构建 abort；`cfg(test)` 下仍 panic 以便单测）；`header_layout` 溢出/非法 layout 亦走 `trap_abort`。
- [x] **自定义 `Eq.eq` 与 Map/Set 键相等分裂**：有 `instance Hash` 的 ADT 上 `==` 强制走 `lumia_eq`（与 Map/Set 一致），忽略发散的 `__Eq_*_eq`；e2e `eq_hash_consistent`。
- [x] **堆软上限 / live-bytes**：`BYTES_ALLOCATED` 在 sweep 时按释放 payload 递减，阈值近似 live set（非 RSS 硬上限；DoS/资源策略另项）。
- [x] **List COW append**：唯一引用 + 余量原地写；别名则几何扩容拷贝；codegen `retain`/`release` 维持唯一性。
- [x] **ADT product COW**：嵌套 ADT/`AdtField`/`Name` 别名 `retain`；`set_field`/浅拷贝 retain 嵌套；`p = p with` → `ensure_unique_consume` 原地写；e2e `adt_with_alias`。栈 LitAdt `with` 升堆克隆。

## 优化与表示（DESIGN / BUILD 下一里程碑）

- [x] **纯互递归 TCO**（DESIGN §4.4）：纯/IO SCC `musttail`；FunRef→`IndirectCall` 在 `funref_locals` 可解析时 musttail（`tco_funref`）；未知闭包仍不 TCO。
- [x] **自动并行**：默认对 FunRef-safe 纯标量 `List.map` 选 `ListParMap`；`List.fold` **仅**在语法上为 `+`/`*`（含顶层 `val add = { a, b -> a + b }`）时选 `ListParFold`（DESIGN：不影响值；非结合如 `-` 回退顺序）；IO/非标量/真捕获回退；`--no-parallel` 关闭。
- [x] **PE `Contains` 假阴性**：仅当集合内**每一个**键/元素均为已知 Int 常量时才折叠；非常量键不再折成 `false`。
- [x] **逃逸分析 → 栈分配 / 多表示 List·Map·Set**：Lit* / LitAdt / 纯 callee 摘要已接线；**晋升** `lumia_list_promote`（concat 空列表恒等返回前将栈 LitList 升堆）。更多表示仍可扩展。
- [x] **部分求值 / specialization（增量）**：§7.5.1-A 增加字面 `ListConcat` / `ListAppend` / `ListTake` / `ListSlice` / `ListReverse` / `AdtTag` 折叠；字面 `Map.get` → `Some`/`None`；Release 在 `inline` 后再跑一轮 `const_fold`；既有 Len/Get/AdtField/Contains。**Int call-site specialization**：`SpecializeConstPass`（`f$c_41` 克隆，开放 `Var` 参数在调用点为已知 Int 时亦可特化）。
- [x] **`std/` 可执行正文**：`std.option` / `std.result` / `std.string` / `std.io` 均为 Source 并经 loader 内联；string/io 薄包装 `lumia_*` runtime foreign（对象 ABI）；`println`/`assert` 仍降为编译器 builtin。

## 工具链

- [x] **`lumia doc`**：CLI 生成 Markdown（`///`、公开 `val`/`type`/`foreign`、`@exports`）；`priv` 默认隐藏。
- [~] **并发 GC**：分代 STW minor 不变；**增量并发 full mark**（worklist + Dijkstra 写屏障着色 + 黑分配 + 收尾 remark）已落地。`--mm=arc` 仍非优先。

## 架构清理（已落地，详见 git 历史）

- 共享前端：`lumia_ty::typecheck_hir`；`lumia` 为 lib+bin（`check_program` / LSP / CLI）。
- `Builtin::info` 元数据（按 family 表 + `may_capture`/`result_heap` 同表）；`builtin_surface`；codegen `CodegenError` + 子状态；`lumia_abi::float_contract`。
- Infer / pkg / lsp / syntax AST 模块拆分；Core `CoreLowerCtx`；`rustfmt.toml`。
- **Builtin 结果 GC 根**：`ResultHeap::{Never,Always,Typed}` 驱动 `roots.rs`（与 `may_capture` 正交；`ListGet`/`AdtField`/`ListParFold` 走类型推断）。
- **LSP semanticTokens**：`lsp/semantic/{token,overlay,walk}`（含 import 路径/别名着色）；编辑器 shared↔vscode 由 `scripts/check_editor_assets.sh` 防漂移。
- **LSP inlayHint**：绑定 / 形参 / 调用与投影结果类型提示。
- **runtime_decls**：表驱动 `RUNTIME_DECLS`（`ENSURE_*` 引用 abi 常量）+ 单测保证每个 `BuiltinInfo.runtime_symbol` 已声明、名字唯一。
- **Trait mangling**：`lumia_hir::mangle_trait_method` 统一 HIR/codegen；`from_method`↔`display_name` 一致性单测。
- **CoreModule::with_functions** / **CodegenTypeTables**；opt 管线 `PipelinePass` 免 `Box<dyn Pass>`；Memo Rust 侧以 `MEMO_TF_*` 为准（C ABI 仍 `lumia_memo_l2_*`）。
- **2026-08-11 深度卫生**：拆分 memo/escape/ty/rt 大测试；abi/rt `type_id` 边界（`map_tid`/`set_tid`）；opt `KnownScalars` 统一；memo lookup 只读借用 + DenseInt 冷 miss 不分配；par_map 去重复 GC inhibit。
- 历史逐项修复列表已并入上方 `[x]` 条目与提交记录，不再在此重复。

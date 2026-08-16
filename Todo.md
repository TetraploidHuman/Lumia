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

- [x] **spawn 返回捕获外层 String 的闭包再 `.concat`**：审计复验已通过（`prepost`；`regress/spawn_string_cap_len`→`prex`/4）。审计须用 `target/debug/lumia`（PATH 旧 wrapper 会误报）。
- [x] **spawn 体内 `contains` / `startsWith` / `endsWith` Bool ABI**：审计复验已通过（spawn 内打印 `true`/`false`）。
- [x] **spawn 体内 `map.values().fold(0.0, +)`**：审计复验已通过（spawn 内 `values().fold` 输出正确）。
- [x] **spawn 体内 `flatMap` 再 `fold(0.0, +)`**：审计复验已通过（spawn 内链式 `flatMap(…).fold(0.0,…)` 输出正确）。
- [x] **spawn 直接返回恒等/多参/插值 Float `Fun`**：`{ x -> x }`、`{ a, b -> a + b }`、`{ x -> "n=${x}" }` 等 join 后调用审计复验已通过。
- [x] **容器中的 Float `Fun` ABI（List/Map/Option/Result）**：浅层 spawn、`flatMap`、嵌套容器、`unwrapOr`/`match`/`alt` 恒等 Fun、`filter` 混恒等、多态 `id`、中间 HOF/`channel.recv`/`joinOpt` 后再 param-`fold` 等 2026-08-16 复验已通过。开放 UFCS 等见独立项。
- [x] **spawn 内 `optionMap`/`andThen`/`resultMap` 后再 `unwrapOr`**：审计复验已通过。
- [x] **嵌套 `andThen` 后再 `unwrapOr`（标量载荷）**：`andThen(Some(3), …)` 等审计复验已通过。**嵌套 Option/Result 见下新项**。
- [x] **`mapErr(Ok(Float))` 后 `match` 载荷 ABI**：审计复验已通过。
- [x] **`unwrapOr(Some/Ok(非恒等 Float→Float Fun))` 再调用**：`{ x -> x + 1.0 }` 路径已好（e2e `unwrapor_fun_float`）；恒等体见上「容器 Fun ABI」。
- [x] **`optionMap`/`resultMap` 恒等 `{ x -> x }` 再提取（Int）**：审计复验已通过（`42`/`7`）。
- [x] **`unwrapOr` 默认空 `listOf()` 毒化 `List[Float]`**：审计复验已通过。
- [x] **spawn 携带 `Bool` 进 Option/Result/List/Tuple/channel**：审计复验已通过（`Some(true)`/`[true,false]`/`(true,1.5)` 均正确）。
- [x] **spawn 返回两个 Float `fold` 之运算结果**：审计复验已通过（外层打印 `6`）。
- [x] **`std` `unwrapOr(None/Err, default)` 对 Float/Bool 默认值 ABI**：e2e `unwrapor_none_defaults` / `unwrapor_err_float`。
- [x] **闭包捕获 `List[Float]` 再直接 `fold(y, +)`（init 为参数）**：e2e `fold_cap_list_float`。
- [x] **`instance Num` 对 Float 字段 product 的 `+`**：e2e `num_vec2_float`（`resolve_trait` 先于 mono，运算符改写为 `__Num_T_add` Call）。
- [x] **channel + spawn 发送具名 `Fun` 绑定**：`val f = {…}; spawn { ch.send(f) }` 经 `collect_fun_cap_tys` 定型 `ClosureCap`，不再与 stamp `Fun` 混成 Int（e2e `channel_named_fun`）。
- [x] **channel 上 `Some(T)`/`None`（及 `Ok`/`Err`）被当成混型**：`channel_hint` 按 tag 重建 Option/Result 参数并用 `Var(MAX)` 合流；e2e `channel_option_result`。
- [x] **闭包捕获后经中间 HOF / 异步来源再 `fold`（Float）**：顺序/融合 fold、`id(xs)`、`MapValues`、`flatMap`/`toList` 累加器、`channel.recv`、`joinOpt() alt listOf()` 等 param-init 路径经 `float_cap_fixup` + identity List passthrough；e2e `fold_hof_param_float`。
- [x] **自递归 + `List[Float].get` / 下标 Float ABI**：mono `ret_ty` 不再把 `sumAt(xs,i,acc)` 收成 `List[Float]`（自递归 Call 周期取 acc ABI；`MonoKey` 仅对 2 参 `f(xs,eps)` 偏 first-List）；e2e `tco_list_float_get`。
- [x] **`List[String]` / 多态绑定上 String 被收成 `List`**：开放 `.len()` / `.concat` 不再默认 `Var→List`（`len_vars` / `concat_vars`）；e2e `string_poly_len_concat`。开放 UFCS 其它方法偏置仍见下项。
- [x] **开放接收者 UFCS 方法解析偏置（多容器同名）**：开放 `.get` 在 `alt` scrutinee 下走 Map/`Option`（`getOr`），否则 List 元素（`pts.get(i)+…`）；`.set` / `Elems` 不默认 List/Map；`.contains` 拒 List；`.isEmpty`/`take`/`drop`/`reverse` 用对应 `*_vars`。e2e `ufcs_open_recv` / `string_open_take_case`。
- [x] **自递归 + `List[Bool].get` 触发 codegen ICE**：`emit_value_if` 在 musttail 后恢复 `root_depth`；e2e `tco_list_bool_get`。
- [x] **嵌套 Option/Result 经多态 `unwrapOr`/`andThen` 提标量 → 错位指针 abort**：`flatten(Some(Some(3)))` / `andThen(Some(Some(3)), id)` / 本地绑定 / `Ok(Ok(3))` flatten / `optionMap(…, { xs -> xs.len() })` 再 `unwrapOr`；mono 不再把 body `Int`（`ListLen`）收成 MonoKey `List`，`value_fixed_ty` 识别 `ListLen`；e2e `nested_option_unwrapor_int` / `nested_result_unwrapor_int`。
- [x] **恒等 Bool `Fun` 经 `alt`（println ABI）**：`(Some({ x -> x }) alt { x -> false })(true/false)` 审计复验打印 `true`/`false`。
- [x] **`mapOf`/`setOf` 字面量不合并 ±0 Float 键**：RT finish + const-fold compact（`known_float` / Neg）；e2e `float_pm0_map_set`。
- [x] **`String` UTF-8 表面契约 vs 字节 API**：`.len()` / `substring` / `.take` / `.drop` / `.reverse` 按 **Unicode 标量**；`toLower`/`toUpper` 用 Unicode case（非仅 ASCII）；`lumia_str_byte_len` 供 println/assert。e2e `string_utf8_len` / `string_take_reverse` / `string_open_take_case`。**仍欠**：字素簇（grapheme）索引。
- [x] **用户和类型：异变体载荷共享类型变量**：sum 参数改为按变体声明顺序**拼接槽位**（非 max-arity 共享）；`Either { Left(a) Right(b) }` 可 `Left(String)|Right(Int)`；`Shape` Circle/Rect 仍可用。e2e `either_mixed_payload`。
- [x] **递归用户 ADT + 递归函数 → `infinite type`**：`Nat { Z S(n) }` / `UList { Nil Cons(h,t) }` 的递归脊用 `Self` 槽（不占类型参数）；拼接槽仅含参数化载荷。无 nullary 基例的树（`Expr { Lit Add }`）靠 **equi-recursive ADT**（`α ~ Expr[α]` 允许；`List`/`Fun`/`Tuple` 环仍拒）。e2e `nat_to_int` / `ulist_sum` / `expr_eval`。
- [x] **`setOf`/`mapOf` 字面量对非 Float 键不去重**：const-fold 压缩 Int/Bool/String/ADT 键（及 RT `finish` 兜底）；e2e `set_map_literal_dedup`。Float ±0 仍见上项。
- [x] **效应并发 Task/Channel（有栈纤程）**：`scope`/`spawn`/`join`/`joinOpt`/`cancelScope`/`channel`/…；Scheduler 标签；Io；e2e `task_*` / `bad_spawn_*`。
- [x] **进程共享堆（§7.7）**：A 盘点 + B 进程 `Heap` + C mutator/memo 根注册 + **D worker/io OS 池**（延迟协程、进程就绪队列）。cargo `lumia_rt` 测例仍 `RUST_TEST_THREADS=1`。
- [x] **Task/Channel 压测入口**：RT `task::stress::*`；e2e `task_pingpong` / `join_tree` / `stress_wide` + multi-worker；`scripts/bench_task.sh`（并入 `bench_all`）。
- [~] **Task/Channel 债务**：ready_home 忙谓词、handoff、sweep 回收等已落地。仍欠（更大设计）— **非 RT `Drop`（C-unwind ABI）**；堆 Mutex 下增量 mark 非真并行。
- [x] **dense-f64 SR bench**：`--no-dense-f64-sr`；mono 泛型形参从 Float clone 升级（避免漏改写时对 IEEE 位做 `smul`）；`cn_hot`/`cn_step` 对标量基线约 100×。
- [x] **mono 漏改写（List/Adt + eps）**：`MonoKey::ret_ty` 优先容器；`call_site_mono_ret` 按 call-site 形参走 `block_result_fixed_ty`（`touch`/`keep`/`l2Normalize` 后 `nAddmm`/`addx` 特化）。
- [x] **mono `id` 包装擦除**：`value_fixed_ty(Call)` 对 Int/Var 返回按实参走 callee 体（`id(b)` 后仍特化）。
- [x] **mono scheme_poly 不升级 ABI**：`upgrade_generic_param_tys_from_clones` 跳过 `scheme_poly`，避免把 `$Float`/`$Bool` 拷回泛型体导致 `dbl(1)`/`id(1)` 误用 Float/Bool println。
- [x] **`block_result_fixed_ty` 自递归**：Call 体展开用 `expanding` 集合防环（`tco_list_sum` 等自调用不再在 core 编译期栈溢出）。
- [x] **channel 热路径**：`full_marking` AtomicBool 镜像；send/recv 空闲时跳过堆 Mutex（join/handoff 仍持锁以保持与 worker 的锁序）。
- [x] **TaskJoin / ChannelRecv Float ABI**：抬升 lambda `List[Float]`/`Adt`/`Map`/`Set`/`Fun` 返回；顶层 `Call`、本地闭包 `icall`+`ClosureCap`、apply/compose/id 形 HOF、`ListParMap`+`ListGet` Float、以及 `spawn { { x -> … } }.join()(…)`；`channel_elem_hint` 从 send 统一 payload。
- [x] **纤程内 ListParMap/ParFold**：活跃 Task 调度时 RT 回退顺序执行（不再 `trap_abort`），避免 spawn 体内自动并行崩溃。
- [x] **`joinOpt`/`recvOpt` Float**：`emit_option_adt_into` 仅对堆 payload `retain`；Float 写 float_mask，避免 COW 把 IEEE 位当指针。
- [x] **COW retain/release/is_unique/drop_alias**：信任 List/ADT 指针 ABI，热路径不再 `is_heap_payload` 抢堆 Mutex。
- [x] **spawn/`match` Float ABI**：`MatchFail` 作 bottom；嵌套 `If` 继承外层 defs；`AdtField` 识别 Float payload；`If`/`match` 返回 `Option`/`Result` 等 ADT 时 `block_result_heap_ty` 合并臂类型；`listOf(Some(1.5))` 等嵌套堆元素保留 elem ADT。
- [x] **spawn `var`/`for`/`while` Float 累加**：`Op::Assign` + `Value::Name` 跟踪 float slot，循环后 `load` 保持 Float ret。
- [x] **spawn 比较/`Task` 嵌套 ABI**：Float 比较/`||`/`!` 标 `Bool`（非 Float/Int）；`TaskSpawn` 保留 `Task[Float]`（`spawn { spawn { … } }.join().join()`）。
- [x] **`fixup_closure_float_caps` 无 cap 也 refresh ret**：directize 后的 `Call`（如 `var f = {…}; f(1.5)`）不再因 early-return 留在 `List(Int)`。
- [x] **channel Bool / Fun / Task hint**：比较/`Bool`、`Fun(Float)→Float`、`Task[Float]` send 保留 elem；`var` Bool 经 Assign/`Name` 保持 Bool ret。
- [x] **同 channel 混型 payload**：检测冲突并在编译期拒绝（`Float` 与 `List[…]` 等同 channel 混发）；异 channel 仍可用 per-local hint。
- [x] **同 channel `Int`+`Float`/`String`**：`note_send` 不再跳过 `Int`，否则只留下非 Int hint，`recv` 把整数当 IEEE 打印。
- [x] **per-channel elem hint**：`channel_elem_by_local` 按 `ChannelNew` 局部跟踪 send；异类型多 channel（Float vs `List[Float]`）各自保留 ABI。
- [x] **map spawn join / list-of-Fun / channel-in-closure Float ABI**：空 `List[Int]` acc `ListAppend` 升级为 `Task`/`Fun`/…；codegen `closure_cap_tys` 预扫描 + `ClosureCap` 继承 AllocClosure 端类型（`spawn { ch.recv() }` 不再对 Float channel `sitofp`）。
- [x] **局部 `{ -> }` thunk channel recv Float**：channel hint 先登记 `ChannelNew` 再传播 ClosureCap（spawn 先于 AllocClosure 列出时也能归到 send）；`ChannelSend`→`Unit`、`ChannelRecv` 经 `by_local` 定 ret；不再把 send/recv 误标成 `List[Int]`。
- [x] **嵌套 `spawn.join` / spawn 捕获 FunRef 的 Float ABI**：`TaskJoin` 从 `Task[T]` 取 T；`ClosureCap`→FunRef 可 directize 成 `Call` 以便 mono；`block_result_heap_ty` 覆盖 `Call`/`IndirectCall`；fixup 接受 Float ret。
- [x] **`spawn { go() }` / map→`List[Fun]` Float ABI**：`refresh_lifted_lambda_rets` fixpoint + 同步 `fun_ret_tys`；`Name`/`Assign`/`ListAppend` 进 `block_result_heap_ty`（含嵌套 Loop）；`prefer_concrete` 细化 `Fun`/`List(Fun)`。
- [x] **curried `compose(f,g)(x)` Float ABI**：`collect_fun_cap_tys` 纳入形参类型；Fun ret 可再细化；`prefer_concrete` 不再让 Float 压掉 Fun；`refresh_alloc_closure_fun_rets` 进 fixpoint。
- [x] **spawn `toMap` / `map.set` Float ABI**：`MapSet` 进 `local_heap_ty`；`List(Int)` 占位让位给 Map；`adt_field_is_float` 穿过 `ListGet`/`Elems`（`toMap` 循环 `p.1`）。
- [x] **spawn `map.remove` / `filter→toMap` Float ABI**：`MapRemove` 定型；`adt_field_is_float` 在 Name 累加器上回退到同函数内 float 字段 `AllocList`。
- [x] **spawn `Map.values` / `take→toMap` / `filter.reverse` Float ABI**：`MapValues`/`MapKeys`/`ListTake`/`Reverse` 进 heap 定型；`list_elem_is_float` 认 Name 累加器；`adt_field_is_float` 穿过 Take/Slice/Reverse。
- [x] **spawn String `.len()` ABI**：抬升把堆 `String`/`concat` 误标成占位 `List[Int]`，codegen 走 `lumia_list_len` 读出 payload 前 8 字节；`local_heap_ty` 认 String/Char/Show/…，`ListConcat` 保 String，`List(Int)` 占位让位给 String。
- [x] **闭包捕获 String 再 `.concat` 的 ret ABI**：`refresh_lifted_lambda_rets` 接受 String/Char；`AllocClosure` Fun ret 同步（否则 `prefix.concat(s)` / spawn 返回闭包后 `.len()` 仍走 list_len）。
- [x] **spawn `Option`/`Result` Float 再 `optionMap`/`resultMap`**：mono 在 FunRef let 时未查 `__lam` ret，`TaskJoin` 被当成 Int，不特化；FunRef/AllocClosure 直读 index，且 fixup 先于 mono。
- [x] **spawn bool `fold`/`and` ABI**：`and`/`or` 脱糖为 `if`；fold 里 `ListGet` 臂未标 Bool 时累加器被清成 Int；识别 `else false` / `then true` 短路形。
- [x] **嵌套 `spawn { join().map(float) }`**：`ListParMap` 进 `local_heap_ty`（回调 Float ret），避免 List[Int] 占位后整数加 IEEE 溢出。
- [x] **`None alt Some(x)`**：DESIGN 要求 rhs 为载荷 T；禁止 Option/Result 作 rhs（曾把 Var 收成 Option，desugar 混 ADT/载荷，Float 打印乱码）。
- [x] **`opt alt float` If 合流**：then=`AdtField` 常为 Int、else=Float 时 `join_value_tys` 曾偏 Int；Int/Var 让位给 Float。
- [x] **`Err(String) alt float` If 合流**：`Err("e")` 的 AllocAdt params 仅有 String，then=`AdtField` 为 String 时曾 `unwrap_or` 成 String，println 把 IEEE 当 Int；`join_value_tys`/`join_heap_tys` 对标量让 Float 赢过 String；`local_heap_ty` 认 `Value::Float`；String ret 可再升为 Float（spawn join）。
- [x] **`channel` 元素泛化破坏 `recv().alt`**：`val ch = channel(1)` 曾 `∀α. Channel[α]`，send/recv 各实例化 α；match 靠模式收紧，alt 见 Var 拒绝。泛化时不量化 `Channel` 下自由变量（value restriction）。
- [x] **`opt alt list` 再 `ListParFold`**：mono 先定 If 再扫臂，If 成 Int，fold 回调停在 Int/Int；先扫嵌套再定型，且 Float init 可回退特化。
- [x] **spawn 调用捕获闭包**：`directize` 把 `ClosureCap→AllocClosure(有 env)` 收成无 env 的 `Call`，LLVM arity 错；仅对无 capture 的 FunRef 目标 directize。
- [x] **spawn 返回捕获 Float 的闭包**：`{ x -> a(x) + 1.0 }` 未把 `x` 标成 Float（只 touch 了 icall 临时）；float Binary 沿 Call/IndirectCall 回溯标参。嵌套 `AllocClosure` 曾把 `ClosureCap` 记成 Int → `sitofp` IEEE；`closure_cap_tys` 多轮合并 Fun。
- [x] **`with` 捕获 ADT / TaskJoin 管道 / flatMap Float ABI**：`ClosureCap` 定型 + `AdtField`/`ListGet`/`Elems`/`ListConcat`/`Binary`；slot 定型用 defs_root；If 臂经外层根解析外局部。
- [x] **`var f = …; f = …` Fun 重绑定**：mut slot COW release 跳过 FunRef（低位 tag），仅释放堆闭包；e2e `var_fun_reassign`。
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

## 性能债（2026-08-15 审计确认）

对照 DESIGN §7 / BUILD §6 与落地项（TCO、自动并行、逃逸 Lit*、Memo、dense_f64 SR、channel 热路径跳锁、COW 不探堆等）。下列为**仍欠的运行时/中后端性能**（不重复架构卫生里的 SR 入侵、阈值硬编码、PipelinePass 双轨等；也不重复上方功能性 ABI bug）。同日晚间已落地若干热路径项（见勾选）。

### 运行时热路径锁与探堆

- [ ] **`is_heap_payload` = 进程堆 Mutex + `heap_set` 查找**：`common.rs` `heap_gen`/`is_heap_payload`。COW 已对 List/ADT 信任 tid；**hash / ord / show / 写屏障 / Map 非 Float 键**仍每点探一次。最大单点税。
- [x] **`lumia_eq` 位相等仍探堆**：`eq.rs` + `may_be_heap_payload_bits` 跳过明显立即数；Map/Set Int 键仍经 `key_eq`→`lumia_eq`（更深的分型 `int_key_eq` 另项）。
- [x] **GC ADT mark 跳过 Float 槽**：`gc.rs` 经 `_pad` / `adt_float_slot`（信任 sanitize）；整波持锁 / 其它边仍探堆另项。
- [ ] **GC `mark_value` 每边再抢锁**：大对象 mark 对每个子字仍 `with_heap`/`is_heap_payload`。宜整波持锁 + 信任已消毒 mask。
- [ ] **shadow-stack `root_push/pop` 每临时都抢堆锁**：`mutator.rs`；热路径大量 List/ADT Let 付 Mutex。宜 TLS 根栈 + 仅 full-mark 时 shade。
- [x] **`GcInhibitGuard` 顺序回退仍进 inhibit**：`list/par.rs` 仅真实并行路径持 guard（`n<64` / task 回退跳过）；原子计数 / append 路径另项。
- [ ] **Memo lookup 整段 `with_heap`**：`memo.rs` 热命中与堆争用。宜世代/epoch，仅 store×full-mark 持锁。
- [ ] **分配路径多次加锁**：inhibit 检查 → maybe_collect → `finish_alloc` 再插 young/`heap_set`。宜单次 `with_heap` 覆盖；nursery bump 延迟入 set。

### 分配与慢路径

- [ ] **`lumia_show` / 嵌套 show 多段 `String` 分配**：容器插值/`println_auto` 锁+分配密集。宜单缓冲写入。
- [x] **dict lookup 每次 `String` 键**：嵌套 `FxHashMap` + `&str` 查询（`dict.rs`）；注册时 `Box<str>`，lookup 零分配。
- [x] **Map overlay `set` 每次建 `Vec` 对**：`map_ops`；小 δ≤8 可栈上写 payload。`map_get` 总堆分配 Option ADT。
  - **部分**：overlay `set` 已改栈上固定数组；`map_get` Option 堆分配仍在。
- [x] **`map_find` 小表扫完全表**：`map_core.rs` 首命中即返回。
- [x] **List take/concat 逐元素拷**：`list/ops.rs` 对齐 `copy_nonoverlapping`。

### 并行与调度

- [x] **`task_runtime_active` 扫全 fiber 表**：`TASK_RUNTIME_USED` AtomicBool + `note_task_runtime_used()`（自 `assert_task_api_allowed`）；阈值/`available_parallelism` 另项。
- [x] **`par_map`/`par_fold` 阈值 `n<64` + 每次 `available_parallelism`**：阈值改为 `n<16`；worker 数 `OnceLock` 缓存。
- [x] **默认 `LUMIA_SCHED_WORKERS`/`IO` = 1**：未设 env 时默认 `available_parallelism`（测例仍钉 `0`/`1`）。
- [x] **纤程默认栈 ~128KiB + spawn 克隆 scope**：默认栈改为 64KiB（`LUMIA_FIBER_STACK_KB` 可覆盖）；spawn 克隆 scope 仍在。

### 中端优化缺口（相对 DESIGN §7.2）

- [x] **无 DCE pass**：`DcePass` 删未使用的非陷阱纯 let（保留 Int 算术/Neg、调用、分配、控制流）；接在 CopyElim 之后（Debug/Release）。
- [ ] **融合仅 fold 汇合；缺 build 侧造林**：HIR 仅 `try_fuse_hof_fold`；`flatMap` 总 materialize；无 `Iota`/`Fused` 表示（DESIGN §7.3）。（`ConcatIdent` 过时注释已改准。）
- [x] **CSE/LICM 禁掉全部 `+−*/%`（含 Float）**：Float 操作数可 CSE/外提；Int 仍禁。Release 在 Inline 后再跑一轮 CSE + LICM。
- [ ] **Inline 仅体积阈值（≤32 ops）且仅 Release**：无热度；`IndirectCall` 不内联；捕获闭包 **恒堆分配**（escape 强制 `AllocClosure`）。
- [x] **空 `setOf()` 不走 Lit**：`repr_select` 空 List→LitList（永生单例），空 Set→HeapSet；对齐需 RT `set_empty` 单例（本次不动 RT）。
  - **收口**：空 Map/Set 与 RT 合同一致，codegen 直接发 **null**（无堆对象）；不必另做 `set_empty` 单例。
- [ ] **默认 Int `+/-/*` 走 `llvm.*.with.overflow`**：仅 `nsw_iv` 形标记免检；`nuw` 未用——一般循环付溢出分支，妨碍向量化。
- [ ] **堆类型 Let 默认 `root_push`**：缺通用 last-use 消根；AdtField→call 仍保守 retain。
- [ ] **通用 `List[Float]` 向量化靠 `dense_f64_sr` 整函数改写**：未匹配形状仍标量 SSA + RT list；SR 是特化逃生舱而非通用向量管线。
- [x] **`dense_f64_sr` clamp 假阳性**：匹配器加 `List[Float]`/`Float`/`ret` 类型守卫；codegen trampoline 拒绝标量 `ret_ty` 残留；回归测 `int_strided_loop_is_not_rewritten_to_clamp`（曾把 `collatzStrided` 改写成 `lumia_f64_clamp` 弄崩 `bench_cpu`）。
- [ ] **编译期：Memo plan ≈ O(n²)**（每候选扫全模块算 reuse）；escape 最多 32 轮×每函数不动点。

## 工具链

- [x] **`lumia doc`**：CLI 生成 Markdown（`///`、公开 `val`/`type`/`foreign`、`@exports`）；`priv` 默认隐藏。
- [~] **并发 GC**：分代 STW minor 不变；**增量并发 full mark**（worklist + Dijkstra 写屏障着色 + 黑分配 + 收尾 remark）已落地。`--mm=arc` 仍非优先。

## 架构卫生（审计确认，未改代码）

2026-08-15 对照源码核实；同日晚间第二轮、2026-08-16 第三/四/五轮深挖补充下方「续」条目。crate DAG 无环、`lumia_abi` 集中契约、vscode↔shared 资产脚本、golden Core、以及下方「Core ABI 收口（第 1 期）」仍健康。下列为**结构/一致性**债务（不重复上方未关的 spawn 语义 bug；也不重复已落地项）。

- [ ] **Core 堆/Float ABI 定型上帝模块**：`lambda_lift/float_abi.rs` ≈3583 行（生产+同文件测试；持续膨胀）；`local_heap_ty` 单函数超长穷举。同层并行 `value_ty::join_value_tys` / `float_abi::join_heap_tys` / `mono/ret_ty::join_fixed_ty` 三套合流近拷贝（Float 优先臂注释互相引用）；**codegen 再有第四套** `closure_cap_tys::prefer_cap_ty`（闭包捕获定型，逻辑同族）。`List(Int)` 作「可能堆」软占位再靠 `prefer_concrete_*` 让位。是反复打 Float/channel ABI 补丁的结构根因——应收成单一 lattice / 表驱动 walker，占位用显式未知类型而非 `List[Int]`。
- [x] **`channel_hint` 测试淹没生产**：测试已外置 `channel_hint_tests.rs`（≈1700 行）；生产文件 ≈600 行。`mono/mod.rs`→`tests.rs`、`hir/lib.rs`→`lib_tests.rs` 同模式已落地。
- [ ] **领域/基准 SR 侵入 codegen + RT**：`emit_value/{collatz,number_theory,trial_div,affine2,float}_sr.rs` 合计 ≈4000+ 行；RT `cn_kernels`/`efe`/`collatz`/`number_theory`/… 再挂一批特化 `#[no_mangle]`（crate 内合计 ≈174）。`name_of`/`is_unit_inc`/`const_of`/`header_lt_*` 等在多份 `*_sr` 复制且签名不完全一致。通用管道被基准形状绑架；应抽共享 pattern 原语，领域内核与语言运行时分层（或标为 optional/bench feature）。
- [x] **`std.cn` / `std.efe` 进入语言标准库**：已迁出为 `extras.cn` / `extras.efe`（`extras/` + `load/std_mod` 白名单；bench 改 `import extras.*`）。根 `.gitignore` 已补 `!/extras/`；BUILD §3 注明发现路径与克隆前提。
- [ ] **`lumia_opt` `dense_f64_sr` 巨型单文件**：≈1918 行整函数 shape 匹配（codegen 双份匹配已消，见下「第 1 期」）。仍缺与其它 `*_sr` 共用的匹配原语；`"lumia_f64_*"` 字符串表继续膨胀时易再漂移。
- [ ] **Core IR 穿透携带 `lumia_hir::Builtin`**：`Value::Builtin` 仍嵌 HIR 枚举（即便已有 `result_ty` stamp）→ `lumia_opt`/`lumia_codegen` 必须依赖 `lumia_hir`+`lumia_syntax`。前端改 builtin 强制中后端重匹配；中后端应只吃 Core 自有 opcode/元数据。
- [x] **抬升 lambda 靠 `__lam_` 字符串身份**：引入 `FunKind::{LiftedLambda,ValGetter}` + `CoreFun::is_lifted_lambda`/`is_val_getter`/`base_name`；lift/val lower 置 kind；`lifted_lambda_names` + `fun_ty_from_tables(..., lifted)`；`float_cap_fixup` 列表参 offset 走 kind 集合（前缀仅作表空时回退）。
- [ ] **foreign 类型面是扁平别名旁路**：`parse_type_name` 只认 `ListFloat`/`ListString` 等单标识符（`infer/module.rs`），无 `List[T]`/`Map[K,V]` 语法；`std/linalg` 与 `extras.cn|efe` 依赖此旁路。与语言表面类型语法分裂，扩展 FFI 只能继续堆别名。
- [ ] **`MonoKind` 无法键化 Task/Channel/Tuple**：`type_to_mono` 对上述（及 Unit/Var）走 `_ => None`；`args_mono_key` 失败则整站跳过克隆。**Fun 已可键化**（`MonoKind::Fun` / `FunRef`）；Task/Channel 仅出现在 `type_is_heap_structure` 恢复路径。多态若以 `Task[T]`/`Channel[T]`/Tuple 为实参，单态管线结构性盲区（与 Float ABI 补丁正交）。Fun 键化残留见第五轮 `unwrap_or(Int)` 污染。
- [ ] **自动并行决策跨 HIR→ty 两阶段**：`list_hof` 在 lower 时先升为 `ListParMap`/`ListParFold`，`lumia_ty::finalize_auto_parallel` 再按 IO/非标量 demote 回顺序 desugar。并行策略散在前端两层，opt/codegen 只见结果；关并行或改安全条件需同时懂 HIR 启发式与 ty 回退。
- [x] **Lit\* / 小容器阈值 `8` 多处硬编码**：已收成 `lumia_abi::SMALL_CONTAINER_MAX`；escape / ReprSelect / RT `MAP_*`/`SET_SMALL_MAX` 共用。
- [ ] **跨层错误类型分裂**：syntax/hir `LocatedError`；ty `TypeError`；core/opt 管线大量 `Result<_, String>`；codegen 公开面以 `anyhow` 为主（另有未贯穿的 `CodegenError`）。诊断易丢 span、调用方无法统一处理。
- [ ] **库路径 panic vs Result 不一**：主路径多为 `Result`，但 `lumia_ty` infer/alt、`lumia_core` lower 等对「理论不可达」仍 `panic!`/`expect`（非 test）。宜统一为 ICE/`Err` 诊断。
- [ ] **双前端管线分叉**：`lumia_core::compile_source_to_core*`（单文件 parse→HIR→ty→Core，供单测）与 CLI/`check_program`（`load` 多文件 + `std.*` + visibility + assert 注解）并行。注释已承认差异；大量 core/opt 单测不经 loader，易漏 std/import/包路径回归。
- [x] **文档幽灵 `--mm=arc` / MmBackend 名存实亡**：BUILD 难度表已改为「愿景」、去掉「`--mm=arc` 另路径」表述；CLI 仍无 `--mm`，BACKEND 仍写死 MarkSweep（实现债保留，文档不再撒谎）。
- [x] **CI 与本地 `check.sh` 纪律分叉**（`RUST_TEST_THREADS` / editor assets）：CI 已 `RUST_TEST_THREADS=1` + Linux 跑 `check_editor_assets.sh`。clippy exclude lumia / llvm-dynamic 等见续条。
- [x] **codegen ADT float_mask 堆/栈近拷贝**：已收成 `emit_adt_set_float_mask`（经 `runtime_fn(ADT_SET_FLOAT_MASK)`）；heap/stack/Option 共用。
- [ ] **`visit.rs` 未成为分析默认入口**：已有 `for_each_local_mut` / `for_each_block_dfs` 等，但 `float_abi` / `channel_hint` / `closure_cap_tys` / 多份 `*_sr` / escape·memo 仍手写嵌套 walker。新 `Value` 臂易漏改；与上帝模块叠加放大维护面。
- [x] **关键字多源词表无单一真源**：真源收成 `TokenKind::KEYWORDS` / `SURFACE_SOFT`；LSP semantic+completion 引用之；IDEA 补 `scope`/`spawn`；`check_editor_assets.sh` 对账 tmLanguage+IDEA ⊇ KEYWORDS。TextMate 仍可高亮 `pure`/`fn`（非 lexer 关键字）。
- [x] **目标平台 Windows vs 工具脚本全 bash/NixOS**：README/BUILD 宣称 Linux+Windows；`scripts/*.sh`、`env.sh`/`install.sh` 钉死 `/nix/store` 与 bash，仓库无 `.ps1` 工作流。Windows 仅靠 CI 装 LLVM SDK，本地脚本路径与宣称平台不对称。
  - **部分**：已加 `scripts/env.ps1` 最小 stub；完整 Windows 工作流仍薄。
- [x] **安装态编译器绑死构建期源码树**：支持 `LUMIA_STD` / `LUMIA_EXTRAS` / `LUMIA_RT_LIB` 覆盖；默认仍回退构建树路径（install 仍不打包 std，但可外置）。
- [x] **`pkg` 版本依赖是假 semver**：`DepSpec::Version` 注释与缺失目录错误写明「非 semver 求解，仅 `./deps|vendor/<name>`」；无 registry/git（能力面仍窄，见 CLI 条）。
- [x] **DESIGN §3.3 与实现类型集漂移**：文档改为 MVP `Int`/`Float`；尺寸变体标为规划。
- [x] **`.gitignore` 根白名单过严**：已补 `!/extras/`（域模块可纳入 VCS）。根目录新增 `LICENSE`/`CHANGELOG` 等仍需显式 `!`；`examples/` 下无扩展名临时二进制仍可能漏忽略。
- [ ] **`lumia_rt` / `lumia_syntax` 公共 API 过宽**：rt `lib.rs` 大量 `pub use` 展开内部模块；syntax `pub use ast::*`。对比 hir/ty/codegen 的 `pub(crate)` 更收敛——重构边界模糊。

### 续（2026-08-15 第二轮；不重复上方条目）

- [ ] **Value→Type 三套完整并行 walker**：除已列的 join/prefer 近拷贝外，`value_ty`（≈955，含 `infer_value_ty_ctx` + 拆出的 `builtin_value_ty`）、`float_abi::{local,block}_*_heap_ty`（`local_heap_ty` 单函数 ≈687）、`mono/ret_ty`（≈720）各自重匹配几乎全部 `Value`/`Builtin` 臂；ABI 补丁常需改三处。应收成单一 typed analysis API，heap/mono/codegen 作薄客户端。
- [x] **lower/ABI 靠魔法迭代上界「收敛」**：`lower` 的 fixup×mono 环改为 change-flag（`specialize` 无新克隆即停）+ `MAX_FLOAT_MONO_ROUNDS=8`；float_abi/fixup 内层上界仍开放。
- [ ] **`ClosureCap.as_float` + `float_cap_fixup` 半吊子通道**：IR 上可变 `as_float` 旗标（`rewrite` 写入 → `float_cap_fixup` ≈1231 行事后补丁 → codegen `emit_calls` 消费），与 `param_tys`/`ret_ty`/闭包捕获表并行。Float 捕获 ABI 应只从 typed cap 表导出，删掉事后 mutation。体量/职责继续膨胀见第五轮。
- [ ] **`mono/specialize.rs` 上帝模块**：≈2135 行集 clone 发现、改写、ret refresh、forwarder 消除、FunRef HOF、Option/Result 载荷规则于一身；几乎每个 mono ABI 修复都落这里。宜按 collect / rewrite / ret_refresh / forwarders 拆分，并与 `ret_ty` 共享 lattice。
- [ ] **codegen `nsw_iv` 第二块基准形岛屿**：`nsw_iv.rs` ≈1071 行（Collatz/`3*x`、fib、matmul 形 peep），经 `emit_fun` 焊进每个函数 emit。与已列 `*_sr` 同病但未收录——通用 NSW 被热核形状绑架。宜迁 opt / feature-gate，codegen 只发 NSW 标记。
- [ ] **Core IR 嵌 `lumia_syntax::{BinOp,UnOp}`**：`ir.rs` 算术节点直接用 syntax token 枚举 → opt/codegen 中后端继续依赖 `lumia_syntax`（与已列 HIR `Builtin` 穿透同族、另表面）。lower 边界应收成 `CoreBinOp`/`CoreUnOp`（或 opcode id）。
- [x] **第五套函数命名协议 `__val_`**：`FunKind::ValGetter` 在 lower 置位；`is_val_getter` 优先于 `__val_` 前缀（前缀仍作过渡）。
- [ ] **Prelude `Option`/`Result` 靠字符串魔改**：`mono/key.rs`、`ret_ty`/`specialize`、`ty/alt`、`ty/infer/expr`、`hir/lower/items` 等处硬编码 `"Option"`/`"Result"` 特判（载荷/擦除/mono）。stdlib ADT 成编译器魔法，非 langitem。宜 prelude 注册表（tag、载荷元数、mono 规则）供 ty/core 消费。
- [ ] **SSA `Local` + 字符串 `Name`/`Assign` 双寻址**：`Value::Name(String)` + `Op::Assign { name }` 与 SSA 并存；ABI/`slot_tys` 必须双轨跟踪。槽位应统一 `Local`/`SlotId`，名字仅调试打印。
- [ ] **`InferValueCtx` 可选表蔓延 / `FunIndex` 仅 mono**：`value_ty` 上下文堆 ≈8 个 `Option<&HashMap<…>>`；`fun_index` 仅 mono 用，而 float_abi/fixup/channel_hint/codegen 反复手拼 `fun_ret_tys`。缺共享 `ModuleTables` → 表装配拷贝。`CodegenTypeTables` 已存在但几乎只服务 codegen（半收口见第五轮）。
- [x] **opt `Pass`→`PipelinePass` 迁移未完 + Release 顺序脆弱**：已删 `trait Pass`；各 pass 为 inherent `run`，管线仅经 `PipelinePass`。Release 多轮顺序仍靠注释（阶段命名/不变量测例另项）。
- [ ] **Builtin→RT 符号在 `BuiltinInfo` 外覆盖**：codegen `builtin/mod.rs` 按 `Type::List` 把 `ListLen`/`MapSet`/`ListGet` 改道 `lumia_list_len`/`lumia_list_set`/`lumia_list_get`（绕开 info 表里的多态符号）。表驱动 emit 被字符串特判挖空；宜把单态分发收进 `BuiltinInfo` 或 Core opcode。
- [ ] **HIR `visit` 未被 `lumia_ty` 使用**：`hir/visit.rs` 已有 `for_each_expr`，但 ty 的 `effects`/`alt`/`parallel`/`product_resolve`/`traits`/`free_vars` 全手写 walker（与已列 Core `visit` 欠债同型、前端侧）。新 `Expr` 臂易漏；ty 应变默认走 hir visit。
- [x] **堆头 / FunRef 标记 / `TRAIT_*` 未进 `lumia_abi`**：`OBJECT_HEADER_BYTES`/`WORDS`、`FUNREF_TAG`、`TRAIT_SHOW…NUM` 进 abi；rt `ObjectHeader` 编译期对账；codegen 栈布局与 dict 注册共用常量。
- [ ] **`runtime_decls` 与 rt `#[no_mangle]` 不对账**：decls 测试只保证「名字唯一 + Builtin `runtime_symbol` 已声明」，不覆盖全量 C 导出（如部分 map/str/dict/memo 计数器可缺席）。非 builtin 直调符号易漏 declare。宜生成或 diff `no_mangle`↔`RUNTIME_DECLS`（测试专用符号白名单）。
- [ ] **RT FFI 边界 crate 级放行「看似 safe」**：`lumia_rt` `#![allow(clippy::not_unsafe_ptr_arg_deref)]`，大量 `extern "C"` 不以 `unsafe fn` 标出。指针契约在类型系统外；UB 审计难。宜收窄 allow、ABI 边用 `unsafe fn` + 薄安全包装。
- [ ] **CI/check 纪律分叉（续）**：双方皆 `clippy --exclude lumia`（CLI/LSP/load 从不 `-D warnings`）；CI Linux 用 `llvm-dynamic`，`check.sh` 不用；`install.sh` 的 `--no-default-features` slim-LSP 产物 CI 未测。在已列 `RUST_TEST_THREADS`/editor assets 之外对齐 feature 与入口 crate。
- [x] **产品版本 / LSP 生命周期无单一真源**（部分）：LSP `serverInfo.version` 已用 `CARGO_PKG_VERSION`；vscode/IDEA 版本漂移与 shutdown/`exit` 行为仍欠。
- [x] **VS Code 对任意 `Cargo.toml` 工作区激活**：已收窄为 `onLanguage:lumia` + `Lumia.toml` + 命令（去掉 `Cargo.toml` / `std/*.lm` / `examples/*.lm`）。
- [x] **根目录探针虽被 ignore 仍占盘**：≈115 个根级可执行文件/`.o`（合计 ≈644 MiB），gitignore 白名单已防误提交，但无 `clean_probes` / 强制写入 `target/` 的约定，`ls` 与误跑陈旧探针仍噪。宜脚本清理或构建只输出到 `target/out`。
  - **收口**：`scripts/clean_probes.sh` 清理根级 ELF/PE 与 `*.o`。
- [x] **DESIGN「语言不提供 pure」与已落地 `foreign … pure` 矛盾**：§1.1 已注明 FFI 荣誉制例外。
- [x] **README GC 表述过时**：已改为分代 STW minor + 增量并发 full mark。
- [x] **Debug 链接不 `--gc-sections` + 跨 profile rt 回退**：Debug 亦 `--gc-sections`/`dead_strip`；跨 profile 默认 `bail`（`LUMIA_ALLOW_CROSS_PROFILE_RT` 可覆盖）。
### 续（2026-08-16 第三轮；不重复上方条目）

对照源码深挖前端 IR、类型约束、中端、codegen/RT 边界与工具链；下列为**新发现**的结构债（不重复已列 Float ABI 上帝模块、SR 入侵、字符串 Fun 协议、双前端、CI 分叉、文档幽灵等）。

#### IR / 类型层

- [ ] **树形 Core 冒充 SSA，无 CFG**：`Value::{If,Loop,Lambda}` 嵌整块 `Block`；`Op` 仅 Let/Effect/Assign/Break/Continue/Return。无基本块图 → 每个中端 pass 自写嵌套 walker；控制与数据同 enum；`Break`/`Continue` 无 loop id，嵌套循环靠 codegen 约定。宜真 CFG（或明确「树 IR + 统一 visitor」并删伪 SSA 叙事）。
- [ ] **中端仍吃开放 `lumia_ty::Type`，无封闭 Core ABI 类型**：`CoreFun::{param_tys,ret_ty}` / channel hint / float_abi 继续用 `Type::Var` 与哨兵 `Var(u32::MAX)`。与已列 `List(Int)` 软占位正交——整条 ABI 合同是 HM 残留而非闭集 ABI。宜 lower 后收成 `CoreTy` lattice，opt/codegen 只认它。
- [ ] **效应三套真源**：`lumia_ty::Effect`（含 Var）、`BuiltinEffect`（Pure/Io）、`Op::Let.pure_region` 驱动 CSE/LICM/折叠；另有 `ty/effects.rs` 事后整树审计。opt 可按 `pure_region` CSE 而不机械绑定 `CoreFun.effect`/`BuiltinInfo`。宜单一效应 IR + 派生标记。
- [ ] **`Scheme` 假类型类袋**：已扩到 **9** 套平行 `*_vars`（`num`/`ord`/`eq`/`len`/`concat`/`contains`/`set`/`elems`/`take`）与真 `trait_preds` 并列；`unify`/`traits::check_*_bind` 每加一类开放方法就复制一套 HashSet 传播。宜统一谓词 IR（`Num(α)` / `HasLen(α)` …），删并行 `*_vars`。
- [x] **类型检查中改写 HIR**：BUILD 明确 **Typed HIR 权威**（`TypedModule` 为语义真源）；rewrite 表仍内嵌于 typed module（未拆出独立 rewrite API）。
- [ ] **`match` 在 typing 前擦成 If**：syntax 有 `Match`；HIR 无 Match 节点（`match_arms`→`If`+`AdtTag`/`MatchFail`）；穷尽性仍吃 `lumia_syntax::MatchArm`。ty 看不见模式；诊断无法挂在 typed Match 上。宜 HIR 保留 Pattern/Match，ty 后再降。
- [ ] **trait/instance 塌成字符串旁表**：HIR `Item` 仅 Fun/Val；trait 数据在 `Module` 映射 → `CoreModule.trait_methods` → `mono/traits` 再解析短名。无结构化 TraitDef；UFCS 改写与 mono stub 易脱节。
- [ ] **表面无类型 AST（注解/FFI 皆 `String`）**：syntax/HIR `ty: Option<String>` / `param_ann`；唯一解析在 `ty` 的 `parse_type_name`。比已列 foreign 扁平别名更广——`List[T]` 与 FFI 别名都只能在 ty 里发明解析。宜 syntax 产出 `TypeExpr`。
- [ ] **Span 死于 Core；`type_at` 线性戳表**：Core `Op`/`Value` 无 Span；诊断中后端多为无位置 `String`；`type_at_span` 倒序扫。宜 Core 带 `Span`/`NodeId`，或诊断只经 typed HIR。
- [ ] **`BuiltinInfo` 非类型规则真源**：info 管 arity/family/effect/emit；真实规则在 `ty/infer/builtins/**` 手写匹配。新 builtin = 元数据 + ty 臂（+ 常再改 ABI walker）。宜表驱动 typing 或从 info 生成。
- [ ] **结构化并发在 HIR lower 抹平**：`scope`/`spawn`→`ScopeEnter`/`TaskSpawn` 等 builtin；ty/opt 不见作用域括号，cancel 嵌套无法结构性校验。
- [ ] **HOF/`for` 大量预类型脱糖**（广于已列 auto-parallel 两阶段）：`list_hof`/`for_loops`/`hof_fuse`/`collections` 在 ty 前冻成循环/builtin；融合形状不可经类型回收。宜保留 HOF 形至 typed 后再降，或把融合推迟到 Core/opt。
- [ ] **积/和双声明、单一 `Type::Adt`**：HIR `adts`+`products`；ty 只有 `Adt` + `ProductState` 旁表。字段/`with`/Show 永特判。宜一种 ADT 模型（或积为无 tag 特化但仍统一）。
- [x] **`Value::Lambda` 抬升后僵尸臂**：`value_ty`/`float_abi` 遇残留 `debug_assert` ICE；codegen 已 bail。
- [ ] **`CoreModule` 是分析黑板**：`hash_adts`/`trait_methods`/`channel_elem_*` 等在 lower 填充、lambda_lift 再改。元数据所有权与「何时权威」不清。宜不可变 `CoreModule` + 旁路 `AnalysisFacts`。
- [x] **Infer 环境用 Int 占位播种**：`listOf`/`mapOf`/`setOf` 已改 ∀ scheme（量化 id 预留 `next_var`）；**`Println` 仍保持 `Int→Unit`**（全多态会让开放 `.get` 接受 Map/`Option` 再毒化算术）。
- [x] **HIR lower API 死字段**：`LowerCtx.ambiguous_product_fields` 已经 `is_ambiguous_product_field` → deferred `AdtField(..., -1, name)` 接线；字段上残留 `#[allow(dead_code)]` 注释过时（可清）。`product_field_owners` 仍死，见第四轮 / 第五轮。

#### 中端 / codegen / RT

- [x] **opt 多处拷贝 `collect_float_locals`**：已收成 `ir_util::collect_float_locals`（DCE/LICM 共用）；CSE 仍自维护 `float_locals`（可后续并入）。
- [x] **CSE 用 `format!("{name:?}")` 键 Builtin**：`ExprKey::Builtin` 改为持有 `Builtin`（`Hash` derive）；不再依赖 Debug 字符串。
- [x] **`CodegenOptions.parallel` 死字段**：已删除；并行仅由 HIR/ty `--no-parallel` 决定。
- [x] **编译选项四散 + Debug 仍跑 `DenseF64Sr`**：`TypecheckOptions`/`InferOptions`/`OptOptions`/`CodegenOptions` + CLI/manifest；`OptOptions::Default` 与 Debug 管线仍开 dense SR。无单一 options 对象，测例/check/build 易脱节。
- [~] **三套调用约定并存**：用户函数仍统一 i64；foreign 已由 `ForeignAbi` 驱动 declare（不再在 codegen 按名字猜）。C vs Runtime marshalling 表仍双份，宜继续收成描述表。
- [ ] **`emit_fun` 函数发射上帝模块（≈833 行）**：帧/根/COW/memo/`dense_f64` 早退/NSW/TCO/`Op` 分发挤在 `emit_function`。宜按生命周期拆（prologue / body / epilogue / 特化出口）。
- [ ] **`Value::Loop` 开放 SR try 链**：`emit_value/mod.rs` 在通用 loop 前串 ≈12 个 `try_emit_*`（与已列 `*_sr` 文件同病，但缺注册表/插件面）。顺序与 fallthrough 隐式膨胀。宜 matcher 注册表或迁出 opt。
- [x] **ADT float_mask 第三发射点**：已并入 `emit_adt_set_float_mask`（task Option 路径同用）。
- [x] **`emit_write_barrier` 死代码**：已删除；注释标明屏障仅在 RT 突变路径，未来直写字段须显式调 `lumia_write_barrier`。
- [ ] **TLS `BACKEND` 空壳罩进程 `Heap`**：`gc.rs` `thread_local! BACKEND` 调 `MmBackend`，真状态在进程 `Heap` Mutex；方法再入 `with_heap`。看似可插拔/每线程，实为进程全局 + TLS 门面（与「写死 MarkSweep」正交）。宜去掉伪装或真做每线程 nursery。
- [ ] **Task ↔ GC ↔ list-par 硬耦合**：GC shade 拉 `task::snapshot_sched_gc_roots`；fiber/channel 调 alloc/root；`list/par` 看 `task_runtime_active()`。三子系统无法独立演化；锁序是跨模块不变量。宜窄接口（根枚举 / 「禁并行」谓词）+ 文档化锁序。**第七轮**：全仓仅两处行内 `heap → sched` 注释，见续「锁序几乎无文档」。
- [ ] **`lumia_opt` 第三前端入口**：`compile_source_to_optimized*` 再调 `compile_source_to_core*`（仍跳 loader/std）。在已列双管线外再添「像完整编译」的捷径。宜只测 Core IR fixture，或强制经 `check_program`。

#### 工具链 / 文档 / 测试

- [ ] **import 整模块内联、无编译单元边界**：`filter_items` 为私有被调者保留整模块；load 合成扁平 `Module`。无增量编译、无库 ABI；菱形只靠 `(file,name)`。宜真正 CU / 导出摘要。
- [x] **`std.*` 发现是编译期 `match`**：按 `std/`/`extras/` 目录发现 `*.lm`（`std.a.b`→`a/b.lm`）；错误列出已知模块；拒绝 `..` 段。
- [x] **每次 `lumia build` shell `cargo -p lumia_rt`**：`LUMIA_RT_LIB` 指向已有静态库时跳过 cargo；否则仍 `cargo -p lumia_rt`（完整预构建随安装分发仍开放）。
- [ ] **LSP 进程级 `Mutex<State>` + Full sync only**：分析仍串在一把锁；已支持 `workspace/configuration` pull + `didChangeConfiguration` push（`lumia.autoParallel`）。multi-root 仍缺。
- [ ] **LSP 功能测跳过 loader**：hover/inlay/semantic 等多走 `check_source`；import/`std`/overlay 回归只能靠真人多文件。宜 loader fixture 测。
- [x] **assert 文案改写仅 build 路径**：`lumia_hir::annotate_assert_messages`；CLI build 与 `compile_source_to_core*` 共用（单文件标签 `"<input>"`）。
- [x] **编辑器 LSP 解析分叉**：IDEA 亦优先 slim `~/.local/lib/lumia/lumia-lsp`（与 VS Code 对齐）；显式 settings 路径仍尊重。
- [x] **VS Code 还对 `std/*.lm` / `examples/*.lm` 激活**（在已列 `Cargo.toml` 之外）：已与 Cargo.toml 一并去掉。
- [ ] **IDE Run/Check 走 CLI shell，分析走进程内 `check_program`**：两套入口、两套 flag；无共享「工程构建」API。
- [ ] **`install.sh` 双二进制靠 `/tmp` 拷贝舞**：先 slim 拷 `/tmp`，再编全量，wrapper 路由 `lsp`。竞态/脆弱（超出「CI 未测 slim」）。宜 cargo feature 两次 `--out-dir` 或 workspace 双 bin。
- [x] **链接器写死 `clang` + 固定宿主库**：`LUMIA_LINKER` 可选驱动（默认 `clang`）；宿主库仍按目标 OS 固定（lld/cl 全量适配开放）。
- [ ] **正确性门四套并行**：e2e（全 CLI）、`opt_correctness`（近克隆 harness）、`golden_core`（无 loader）、RT `task::stress`。loader/std/import bug 易漏 golden；harness 逻辑重复。宜一条「程序管线」测 + 分层夹具。
- [ ] **`bench_cn_*.sh` 近克隆骨架**：hot/step/efe/fuse/forward/strict 同构；维护随领域 bench 线性涨（结构债，不止「cn 进 std」）。宜共用 `bench_measure` 驱动。
- [x] **DESIGN 仍列未实现表示**：`Fused`/`COWList`/`SortedTree`/`BuildFused` 等（§3.5/§7）；Core `ListRepr` 仅 Heap/Lit，Map 无 Sorted/BuildFused。宜标「规划」或删表，避免读成已选型。
  - **收口**：DESIGN §3.5/§7 已标注已落地 vs 规划。
- [x] **BUILD 称 `is_heap_payload` O(1)**：已改为「堆 Mutex + `heap_set` 查找」。
- [x] **BUILD「semispace 易换」**：见上（难度表愿景化）。

#### 补遗（同轮复核；不重复本轮已列）

- [ ] **`Type`/`Effect` 住在 `lumia_ty`，Core 硬依赖推断 crate**：`lumia_core`→`lumia_ty`；IR 直接嵌 `lumia_ty::{Type,Effect}`。与已列「收成 `CoreTy`」互补——即便有 `CoreTy`，抽 `lumia_types`（或 abi 旁路）才能让 opt/codegen 不绑 HM。宜类型定义与推断分 crate。
- [ ] **和类型 `sum_max_arity` 垫成统一 `params` 向量**：lower 算最大变体元数；ty/`value_ty`/`mono`/`AdtField` 按此垫 `Type::Adt.params`。这是上方「异变体载荷共享类型变量」的**表示根因**（Prelude Option/Result 靠字符串特判绕开）。宜 per-variant payload，勿 max-arity 积。
- [ ] **`lambda_lift` 名不副实，实为 ABI 厨房**：目录以 `float_abi`/`channel_hint`/`float_cap_fixup` 为主（合计数千行），真 lift（`rewrite`/`captures`）反而少数；`mod.rs` 还 re-export hint/fixup。与已列上帝模块正交——是**包边界撒谎**。宜拆 `lift` vs `abi_refine`。
- [ ] **`lower_hir` 编排中端遍，而非纯 HIR→Core**：末尾串 lift→hint→directize→trait→6×(fixup+mono)→stubs。与已列「魔法迭代上界」同管线、但是**所有权**债。宜 lower 纯翻译；具名 Core pass 管道 + 阶段不变量。
- [ ] **Escape / Lit\* repr 所有权骑 core↔opt**：`escaping` 与 `*Repr` 在 core 定义并默认 `Heap*`；真正填充在 opt Escape/ReprSelect。opt 前 Core「合法但不完整」。宜 opt-only 注解或显式「after escape」阶段类型。
- [x] **foreign 调用约定靠 `lumia_` 名字前缀**：已加 `ForeignAbi`；lower/synth 经 `from_symbol` 写入 `CoreFun.foreign_abi`，codegen 只读字段。符号约定仍在 lower 边界一次解析。

### 续（2026-08-16 第四轮；不重复上方条目）

前端可见性/泛化、RT 容器表示、LSP overlay、包信任与编辑器契约。下列为**新发现**（不重复已列双前端、假 semver、关键字多源、厨房 `lumia` 库、第三轮 IR/效应/float_mask 等；亦不重复本轮已勾选落地项）。

#### 前端 / 类型 / 诊断

- [x] **`priv` 在 HIR 被抹掉**：`Fun.is_priv` / `Item::Val.is_priv` 从 syntax `ValItem.is_priv` 拷贝；`NameVisibility` 仍由 loader 驱动，HIR 可表达隐私供 IDE/单文件路径。
- [x] **穷尽性跳过 trait/instance 方法体**：`check_module_matches` 亦扫 `Trait`/`Instance` 方法体（与 `Val` 同）。
- [x] **`trait` 必须源码先于 `instance`（`type` 无此限）**：`collect_instances` 两遍（先注册 trait 再校验 instance）；顺序无关。
- [ ] **互递归多态随声明序**：`infer_module_inner` 先绑 mono 占位，再按 item 序 generalize/`bind_scheme`。靠前函数见 mono 占位、靠后见 scheme——经典 HM 债，无 SCC 不动点文档/实现。
- [x] **`Scheme.eff_vars` 死字段**：已从 `Scheme` 删除；效应仍单态/用点新鲜（与「效应三套」正交）。
- [x] **僵尸 `BinOp::And|Or` 定型臂**：HIR 已把 `and`/`or` 降成 `If`；ty 遇残留臂报错（不再假装可定型）。
- [x] **FileId 事后 stamp；诊断格式化无视 `Span.file`**：`format_diagnostic_files` 按 `span.file` 取 path/src；loader/`diag_err` 走文件表；单文件 API 仍要求调用方 stamp/`with_file`。
- [x] **`${…}` 插值 span 相对片段（定位错）**：`StringPart` 带 `abs_start`；片段 parse 后 `offset_expr`/`shift` 回文件坐标；错误 span 亦绝对。
- [x] **HIR `Fun`/`Val` 无声明 span**：`Fun.span` / `Item::Val.span` 取自 syntax `ValItem`/`Foreign`；`decls` 用声明 span。
- [ ] **Span 键 rewrite/事实表会撞**：`ufcs_rewrites`/`alt_kinds`/字段/`with` 均 `HashMap<Span,_>`。同 span 静默覆盖；宜 `NodeId`。
- [ ] **表面糖在 parser 抹平**：`a..b`/`a to b`/裸 `{ it }` 在 parse 成 Call/Lambda；syntax AST ≠ 书写面；fmt/IDE「原样」丢失。宜 typed/HIR 脱糖阶段。
- [x] **软关键字 `to`（Ident，非 TokenKind）**：现为硬关键字 `TokenKind::To`（KEYWORDS/LSP 可对账）；中缀糖与 `to(…)` 主表达式仍产出 `Ident("to")`。
- [ ] **仅 item 级恢复 + 列 0 同步启发**：`parse_module_recovering`/`synchronize_item`；无表达式级恢复。一处坏表达式可吞整项。
- [ ] **`bump` 每步 clone 带 String 的 Token**：无 intern/arena。解析所有权模型偏重。
- [ ] **Lower 错误 `RefCell` 先错即终**：`set_err` 仅在空时写入；嵌套失败丢弃。无法多诊断 lower。
- [ ] **双类型打印机 + unify 行话**：`Display for Type`→`?N`；IDE `display_type` 接地+字母名；unify 发 `infinite type` / `Debug` mismatch。违 DESIGN §3.2 用户面措辞。
- [ ] **`join` 按元数重载 Task vs List**：`from_method` `(join,1)→TaskJoin`、`(join,2)→ListJoin`。同名两 builtin，易误解析。
- [ ] **`show_methods` 仅 Show 旁路表**：在通用 `trait_methods` 外再特判 Show。其它 trait 无对称快路径——又一层魔法。
- [ ] **一切积/和盲插 `Eq`/`Show` instance**：`collect_instances` 对所有 product/ADT（含 prelude）插入。派生策略非 langitem/注册表。
- [x] **`product_field_owners` 同属死诊断通道**：已删；歧义仍靠 `ambiguous_product_fields`（ty 解析）。
- [x] **词法接受再拒绝的 `=>`**：`=>` 现为 `TokenKind::Error`（不再有 `FatArrow`）。`..=` 仍 lex 为 `DotDotEq` 以便 parser 给出定向移除提示。

#### RT / opt / codegen

- [x] **Hash vs 线性靠 payload `size` 启发式，无表示 tag**：`TID_HASH`（bit 11）；`map_alloc_hash_tid`/`set_alloc_hash_tid` 打标；`map_is_hash`/`set_is_hash`（及 mark）读 flag；demote→线性时 `tid_without_hash`。
- [ ] **三套互不兼容的「持久更新」模型**：List/ADT 头 `rc` COW；Map Overlay（`count==-1`、无 RC）；Set 总是整表拷（命中 contains 仍 memcpy）。无共享持久容器层。
- [x] **空值表示分叉**：空 List→永生单例；空 Map/Set→**null**。ensure/len/dispatch 永特判 null（与已列「空 setOf 不走 Lit」互补——是表示契约分裂）。
  - **收口**：接受契约分裂；codegen 空 Map/Set 发 null（不再堆分配空对象）。统一单例若要做需另开。
- [ ] **Map/Set 开哈希近克隆**：`MAP_ST_*`/`SET_ST_*`、`*_hash_find_slot`/`*_from_linear_to_hash`/`*_finish` 平行拷贝。宜参数化一张表实现。
- [x] **「nursery」名不副实**：文档写 nursery；实现是 `alloc` + `h.young.push`（无 bump 区、无延迟入 set）。与已列「分配多次加锁」愿望正交——当前根本不是 bump nursery。
  - **部分**：`gc`/`common`/`heap` 注释改为「young generation list / 非 bump nursery」；bump 实现仍欠。
- [x] **`HEAP_REBORROW` / `SCHED_REBORROW` 双份不安全重入**：`rt/reentrant::with_mutex_reentrant` 统一 heap/sched。
- [ ] **Memo 存 TLS、堆是进程全局**：`MEMO_TF` TLS + `MEMO_REGISTRY` 供 GC 扫；OS worker 间不共享命中。与 `PROCESS_HEAP` 不对称。
- [ ] **Memo 规划无视 `IndirectCall`/FunRef**：`plan.rs` 只认 `Value::Call{fun:name}`。HOF 站点永不进 Slots——相对 FunRef ABI 栈结构性盲。
- [x] **Memo 坏 `fun_id` 软返回 0（像 miss）**：`lumia_memo_l2_*` / `lumia_memo_idx_*` 对越界 `fun_id`（及 null out / idx 越界 store key）`trap_abort`；稠密 key 域外 lookup 仍为 miss。
- [ ] **Escape 摘要键为函数名字符串**：`HashMap<String, ParamEscape>`；mono/`$c_` 改名是静默摘要键风险（与已列 Fun 字符串协议互补）。
- [ ] **目标三元组锁宿主；`.o` 留在产物旁**：`compile_module` 默认 triple+宿主 CPU，写出 `.o`/`.obj` 后 clang 链接且不删；无「只出对象不链」。根目录探针噪音的又一来源。
- [ ] **workspace Inkwell 钉死 `target-x86`**：非 x86 宿主结构性出局（即便 `initialize_all`）。
- [x] **Option tag 挂在 `CodegenOptions` 而非 Core**：lower 写入 `CoreModule::{option_some_tag,option_none_tag}`；codegen 从 core 读取，已从 `CodegenOptions` 删除。
- [x] **调度器 kind 魔数未进 `lumia_abi`**：`SCHEDULER_WORKER=1`/`IO=2` 在 `lumia_abi`；RT `sched_core` re-export。
- [x] **`lumia_abi` 塞了 `workspace_root` 路径助手**：已迁 `lumia::paths`；测试/codegen 本地 `../..` 辅助。
- [ ] **`lumia_rt`/`opt`/`core` 无 Cargo feature**：领域核/SIMD/stress 无法包级裁剪；静态库永远全量（与已列 SR 入侵互补——缺门闩）。
- [ ] **RT 测例半迁**：已有 `crate_tests/{eq,gc,list,…}`，大量 `#[cfg(test)]` 仍嵌生产文件（同 channel_hint/scheduler 淹没模式，RT 内未完成拆分）。
- [ ] **`env.sh` 版本钉死 Nix LLVM 21.1.8 store glob**：所有 check/e2e/install/bench source 它；非 Nix 仍扩 `/nix/store/*`；升 LLVM 必改钉（在「Windows vs bash」之上的版本耦合）。
- [ ] **`examples/` 扁平回归堆**：≈244 顶层 `.lm` 混 `bad_*`/`bench_*`/`task_*`/教程；仅 `regress/` 成类。e2e 指进这锅汤。宜 `examples/{guide,reject,bench,task}`。

#### LSP / 包 / 编辑器 / CLI

- [x] **多文件失败被单缓冲恢复掩盖**：`analyze_buffer` 在 load/typecheck 已有诊断时优先发布之，不再被 `check_source_recovering` 吞掉。
- [x] **跨文件诊断发到错误 URI**：`analyze` 按 `Span.file` 对应 `path_to_uri` 分批 `publishDiagnostics`；编辑缓冲无本文件错时清空 stale。
- [x] **依赖变更不重分析**：处理 `workspace/didChangeWatchedFiles`；任意 `.lm` 变更时重分析全部打开缓冲。
- [ ] **按 URI「当入口」改变可见性**：单独打开库文件 → `entry_file`=它；作为 import 则否。同文件诊断/hover ≠ 真入口包检查。
- [ ] **overlay 键经 canonicalize，loader `get` 路径身份脆弱**：符号链接/未规范化入口/未保存路径可 miss overlay。
- [x] **LSP 严重级别恒 Error；无 code/relatedInformation/tags**：按消息前缀填 `code`（`parse`/`lower`/`type`）；severity 仍为 Error（尚无 Warning 面）；relatedInformation/tags 仍缺。
- [ ] **多文件 fail-fast 单诊断 vs 恢复路径多诊断**：CLI/LSP 多文件 `typecheck_hir`；缓冲恢复 `typecheck_hir_recovering`。体验分裂。
- [x] **`lumia doc` 把 byte offset 当「行」打印**：`doc.rs` 经 `line_starts`/`byte_to_line_col` 输出 `file:line:col`。
- [x] **LSP format 末端列用 `lines().last()`**：改为 `byte_to_line_col` 算 EOF（尾随换行正确）。
- [ ] **LSP 能力面缺口大**：无 references/rename/signatureHelp/codeAction/highlight/workspace symbol/call hierarchy/folding/cancel；不支持方法直接 `-32601`。`initialize` 忽略 client capabilities。
- [x] **补全无视光标**：按 `position` 取 `prefix_at` 前缀过滤（大小写不敏感前缀）；成员/import 上下文仍粗。
- [x] **文档符号含内联 import 项且 range 相对 file-0**：outline 跳过非本文件 `Fun`/`Val`（按声明 `span.file`）；`decls` 用声明 span。
- [x] **Check/Build clap 旗标手写双份**：`--no-parallel` / `--trust-foreign-pure` / `--no-trust-foreign-pure` 收进共享 `SharedCheckArgs`。
- [ ] **无 `lumia run`；`pkg` 仅 init/lock/add**：BUILD 能力表与 CLI 表面仍不齐（`fmt` 零文件已改为报错退出）。
- [x] **`Lumia.toml` 不 `deny_unknown_fields`**：`Manifest`/`PackageMeta`/`DepTable` 已 `deny_unknown_fields`；拼错 `trust`/`link` 键会解析失败。
- [x] **`verify_lockfile` 跳过根 `path=="."` 且忽略多余 lock 包**：根版本亦校验；lock 中多余包名报错；单测 `verify_lockfile_checks_root_version_and_rejects_extras`。
- [x] **`trust_foreign_pure` 清单 OR 粘滞、无 CLI 关闭项**：`Option` 覆盖（CLI flag / `--no-trust-foreign-pure`）；无 flag 用包设置；LSP 同默认；loader 对包级 trust 打 warning。
- [x] **IDEA「Build File」写 `$dir/$stem`，Run/VS Code 用 `target/lumia/$stem`**：Build File 与 Run 对齐到 `target/lumia/$stem`。
- [x] **VS Code client 仅 `file` scheme**：`documentSelector` 含 `untitled`（未保存缓冲可挂 LSP）。
- [x] **IDEA `resolveProjectEntry` 回退 `examples/hello.lm`**：仅 `src/main.lm` / `main.lm`，否则聚焦 `.lm`；不再误用仓库 hello。
- [ ] **IDEA liveTemplates 是第三套片段**（不经 shared→vscode 对账）。与已列关键字多源同病、片段面。
- [ ] **bench 测量骨架在 `bench_cn_*` 外仍克隆**：`bench_cn_vs_torch`/`bench_memo`/`bench_cpu` 本地 `measure_*`/`stats_*`；`bench_memo` 用 `target/debug/lumia`、其它偏 release。宜一律走 `bench_measure.sh`。

### 续（2026-08-16 第五轮；不重复上方条目）

对照源码复核已勾选项、刷新过时度量，并补包装/半收口/mono 键化残留等**新发现**（不重复上帝 `float_abi`、三套 walker、SR/`nsw_iv`、双前端、假 semver、LSP overlay 等已列项）。

- [x] **`extras/` 被根 `.gitignore` 白名单漏掉**：已补 `!/extras/`；`cn.lm`/`efe.lm`/`README.md` 纳入 VCS。BUILD §3 交代可选域模块与发现路径。
- [x] **文档未交代 `extras` 包装契约**：BUILD §3 workspace 树已列 `extras/`（非语言 std、bench/`import extras.*`、须随 clone 检出）。
- [ ] **`float_cap_fixup` 膨胀为第二 ABI 上帝模块（≈1231）**：远超原「`as_float` 半吊子通道」叙事——已吞 `refresh_lifted_lambda_rets` / `refresh_alloc_closure_fun_rets` / `upgrade_captured_list_fold_float` / call-site List 升级等；**零 `#[cfg(test)]`**。宜拆 `abi_refresh` 并入 typed cap 表，或外置测例并冻结行数。
- [ ] **`CodegenTypeTables` 半收口、`ModuleTables` 仍不存在**：「架构清理」已记 `CodegenTypeTables`，但生产路径几乎只在 codegen `emit_fun/helpers` + `closure_cap_tys` 使用；`float_abi`/`float_cap_fixup`/`channel_hint` 仍每遍手拼 `fun_ret_tys`（fixup 内多次重建）。代码中无 `ModuleTables` 符号——是未完成迁移。宜共享模块表 API，或删掉仅语法糖的包装以免叙事超卖。
- [x] **`MonoKind::Fun` 键化后开放子类型 `unwrap_or(Int)` 静默污染**：`type_to_mono` 对 Fun 形参/返回子键失败时整键 `None`（与 List/Map/Adt 一致）；单测 `args_mono_key_rejects_fun_with_open_param` / `args_mono_key_accepts_ground_fun`。
- [x] **`MonoKind::FunRef::to_type` 仍映成空参 `Fun([], Int)`**：改为 `Unit` 哨兵；`param_tys`/`ret_ty` 经 `CoreFun` 表解析真 Fun；单测 `funref_to_type_is_not_fake_zero_ary_fun`。
- [ ] **`lambda_lift/heap.rs` 第四套「是否堆」启发式**：lift 用 `block_result_may_heap_with_params`（Builtin 白名单跳过 `ChannelRecv`/`TaskJoin` 等），与已列 `value_ty` / `float_abi` heap_ty / mono `fixed_ty` 并行。新 Builtin 易漏。宜并入单一 heap lattice，lift 只读 `ret_ty`/policy。
- [ ] **`runtime_decls.rs` 手维百科 ≈1064 行**：在已列「与 `no_mangle` 不对账」之外——表本身成巨型单文件，每加 RT 导出就手工追加。宜从 `lumia_rt` 导出生成/diff，或按子系统拆表并强制 CI 对账。
- [ ] **`scheduler.rs` 生产面膨胀（≈1254）**：不再被测试淹没（≈326 测试），但调度/亲和/env/GC 根快照仍挤同一文件；与已列 Task↔GC↔par 耦合叠加。宜按 queue/affinity/pool/roots 拆模块。

### 续（2026-08-16 第六轮；不重复上方条目）

对照源码再审计（core/opt/codegen + ty/hir/syntax/rt/CLI/LSP）；下列为**新发现**或对已列项的明确扩展（不重复上帝 `float_abi`、三套 walker、SR/`nsw_iv`、双/三前端、假 semver、LSP overlay、`Scheme` 袋本身、`ModuleTables` 半收口总述等）。

#### 命名协议 / 契约边界

- [ ] **HIR 脱糖合成名成第六套（+）命名协议**：`list_hof`/`collections`/`hof_fuse`/`for_loops` 生成 `__map_acc_*` / `__fmap_acc_*` / `__tolist_acc_*` / `__fuse_acc_*` / `__fold_x_*` / `__i_*`；`float_cap_fixup` 用 `starts_with` 白/黑名单、`channel_hint` 用 `contains("__map_acc")` 消费。改脱糖前缀会静默改 ABI。与已列 `__lam_`/`__val_` 同族但未收录。宜 `LocalKind`/`SlotRole`，禁止中端解析 `__*_acc` 字符串。
- [ ] **Inline 再引入 `$inl{tag}_` 槽名**：`opt/inline.rs` 重写可变槽为 `$inl…`；与 `$` mono / `$c_` 共用 `$` 命名空间。宜 inline 用 `Local` 重编号，勿改写字符串槽名。
- [x] **`lumia_core` 完全不依赖 `lumia_abi`**：core → abi；再导出 `SMALL_CONTAINER_MAX`；`FunKind`/`base_name` 与契约层同仓演进。
- [x] **`mono_of` 半迁移：查找仍 `split('$')`**：`CoreFun::base_name` + key/`unwrapOr` 跟踪优先 `mono_of`；`strip('$')` 仅作无表回退。

#### 双轨 / 近拷贝 / 包边界撒谎

- [x] **ADT「递归脊柱」分类算法双份**：`lumia_hir::{classify_sum_field_recursive,sum_parametric_arity}`；ty/core lower 共用。
- [x] **`mono/traits.rs` 名不副实**：已拆 `mono/directize.rs`（`directize_funref_calls` / `directize_block`）；`traits.rs` 保留 resolve + Num Binary→Call + stubs。`specialize` 改依赖 `directize`，打破模块环。
- [ ] **双轨函数特化：类型 mono（core）× 常量 specialize（opt）**：`mono/specialize`（`$Float`/`MonoKey`）与 `SpecializeConstPass`（`$c_`）两套 clone/改写 Call；Release 交错顺序靠注释。宜统一 Specialization 框架，或阶段不变量测例。
- [x] **Num 实例双路径**：删除 codegen `try_emit_num_override`；残留 Num ADT Binary 在 `emit_arith` ICE（须先经 `resolve_trait_method_calls`）。
- [ ] **未知类型普遍 `unwrap_or(Type::Int)` / `ground_open_vars: Var→Int`**：与已列 `List(Int)`「可能堆」软占位 **正交**——这里是「未知→标量 Int」，错误方向相反。宜显式 `CoreTy::Unknown`，禁止 Int 作缺省。
- [x] **`closure_cap_tys` 预扫填空表 + 魔法 `0..8`**：预扫注入 `core.sum_max_arity` + `fun_param0_identity`；`0..8` 标明为 change-flag 上界。
- [ ] **`FunTables` 成 codegen 侧第二块 Core 黑板**：镜像 `hash_adts`/`sum_max_arity`/`channel_elem_*`/`adt_variant_names` 并自建 `fun_ret_tys`/`closure_cap_tys`。权威在 CoreModule vs FunTables 不清。宜只读 `AnalysisFacts`/`ModuleTables`，FunTables 仅 LLVM 句柄。
- [x] **Core/codegen 仍留 `BinOp::And|Or` 僵尸臂**：ty 已拒残留；Core/`emit_arith` 遇残留 debug_assert / ICE（不再当 Binary 求值）。

#### 测试结构 / 死 API / 过宽 pub

- [~] **上帝生产模块零同文件测试（mono 溺测的镜像）**：`mono/mod.rs` 巨型同文件测已外置 `mono/tests.rs`；`specialize`/`float_cap_fixup`/`ret_ty`/`key`/`traits`/`rewrite` 等仍 0 近距测。宜继续按子模块外置 + 行数预算。
- [x] **死双轨 API：`lower_hir` / `default_list_repr`**：生产皆 `lower_hir_with_schemes`；`lower_hir` 仅定义+re-export。`default_list_repr` 仅自测，ReprSelect 直接写 `HeapList`。宜删或 `#[deprecated]`。
  - **收口**：已删二者。
- [x] **opt 公开 Pass 类型 + 再导出 abi `MEMO_*`**：`pub use EscapePass/InlinePass/…` 可绕开 `PipelinePass` 手跑；MEMO 常量经 opt 成第二入口。宜 `pub(crate)`；MEMO 只从 abi 取。
  - **收口**：Pass/`plan_memo_*` → `pub(crate)`；MEMO 常数不再经 opt 再导出。
- [x] **`pass_names` 把 `memo_tf` 挂在末尾，实际最先跑**：`optimize` 在 CSE 前 `plan`/`apply`；`pass_names` 仍 `push` 到尾且测试只 `contains`。工具/诊断顺序撒谎。宜名字列表反映真实顺序。
  - **收口**：`pass_names(true)` 以 `memo_tf` 为首；单测断言顺序。
- [x] **间接调用 callee 降失败时静默 `Value::Int(0)`**：`lower/expr/call.rs` 在 `lower_expr(callee)=None` 时造假函数指针。异于「未知→Int」类型缺省——是 **lowering 毒化局部**。宜 ICE/`Err`。
  - **收口**：记 ICE 并中止该 Call；`lower_hir_with_schemes` → `Result`。
- [x] **Core lower `Alt`/`With`/`Unit` 用 `panic!`/`expect`**：`control.rs` 显式 panic；管线对外仍 `Result<String>`。坐实已列「panic vs Result」的 **Core lower 具体面**。宜一律 `Err(Ice)` 上抛带 span。
  - **部分**：Alt/With → `note_ice` + `Err`（带 span）；部分 `expect` 仍留。
- [x] **`check_program` ↔ `check_program_with_overlays` 近克隆**：同构 load→lower→typecheck，仅 overlays/错误包装不同。异于双前端——是 **CLI 库内重复**。宜共享 `typecheck_loaded`。
- [x] **`corosensei` 未进 `workspace.dependencies`**：`lumia_rt` 单独 `"0.3"` 浮动。宜迁入 workspace 并钉小版本。
  - **收口**：workspace `corosensei = "0.3.4"`。
- [x] **BUILD §3 对 `lumia_rt` 描述过窄 + DESIGN/BUILD「SSA/基本块」措辞**：文档仍写「GC + println*」与「Core SSA / 基本块」；实现含 Task/Channel/memo/域核，IR 为树形嵌套 Block。宜改正 RT 能力表；文档改为「树形 ANF / 伪 SSA」或标规划 CFG（第三轮实现债的**文档面**）。
- [x] **`dump_fold_diag`：名为测试、实为 `eprintln` 转储**：`#[test] dump_cases` 零 assert；污染 `cargo test -p lumia_opt`。宜删或 `#[ignore]` 诊断 bin。
  - **收口**：已删该模块。
- [x] **锁序几乎无文档（证据加细）**：全仓仅 `gc.rs`/`scheduler.rs` 两处行内 `heap → sched`；memo/dict/mutator/channel 未入表。坐实第三轮「Task↔GC↔par…宜文档化锁序」。宜 `LOCK_ORDER`（或 `rt` 模块文档）+ 可选 CI 禁令。
  - **部分**：`lumia_rt` crate 文档增加 **Lock order**（heap → sched）；CI 禁令仍欠。

#### 前端 / RT / 包装 / 文档

- [ ] **`std.linalg` 仍占语言标准库**：`cn`/`efe` 已迁 `extras/`，`linalg.lm` 仍几乎全是 `foreign`→`lumia_f64_*`，且在 `std_mod` 白名单。域模块迁出不完整。宜迁 `extras.linalg`（或等价），RT 域核走 feature。
- [ ] **RT `dispatch.rs` = 开放方法的运行时孪生**：`lumia_len`/`concat`/`set`/`elems`/… 按 `type_id` 分发，与 ty 的 `*_vars` 同族语义分属两处。宜单一能力表生成/对账。
- [ ] **`string_io.rs` 混装 String / IO / stdin / trap**（≈498）：核心字符串表示与 I/O 策略、trap 耦在一起。宜拆 `string` / `io` / `trap`。
- [ ] **前端巨型分发入口**：`infer_module_inner` / `hir/lower_expr` / `parse_primary` 各两百行级总 match（syntax `expr.rs` 整文件 ≈753）。新糖/项种类都挤同一臂。宜按族拆文件 + sugar 独立 pass。
- [ ] **LSP semanticTokens（及 format）对已分析缓冲二次 `parse_module_*`**：`Analysis` 不缓存 syntax AST；着色走未 rewrite 表面树、靠 span 对 typed。与「typed HIR 权威」分裂叠加。宜缓存 AST 或明确「着色只认表面」。
- [x] **hover 硬绑 `span.file == 0`**：`Analysis.buffer_file`；hover/outline 按该 id 过滤（不再假定 entry 恒 0）。
- [x] **用户 import 三套路径约定无文档**：BUILD 写明 `a/b.lm` → `a/b/mod.lm` → `a.b.lm`；`std`/`extras` 走目录发现。
- [x] **`item_file` / `item_file_id` 双份**：收拢为 `vis::item_file`；load 复用之。
- [x] **BUILD「前端统一」叙事超卖 + Pass 列表过时**：§3 改为「共享 typecheck；完整管线仅 `check_program`」；Pass 列表对齐 DEBUG/RELEASE_PASSES。
- [x] **`annotate_assert_messages` 手写全量 HIR walker**：经 `for_each_expr_mut`；实现迁 `lumia_hir::annotate_assert_messages`。
- [x] **MIT 声明无 `LICENSE` 文件**：根 `LICENSE` + `.gitignore` `!/LICENSE`。
- [x] **空 `codegen/src/bin/` + 工作区级 clippy 放行**：已删空 `bin/`；工作区级 clippy allow 收窄仍开放。
- [ ] **`opt_correctness` 与 e2e harness 近克隆**：各自 `workspace_root`/`lumia_bin`/`build_and_run`。坐实已列「四套正确性门」的骨架分叉。宜抽 `tests/common`。

### 续（2026-08-16 第七轮；不重复上方条目）

对照源码再挖 IR 半废弃路径、mono/opt 内部结构、RT 并发惯例、CLI/编辑器/文档与测试组织；下列为**新发现**（不重复上帝模块、三套 walker、SR 总述、双/三前端、LSP overlay、命名协议总述、`Scheme` 袋、`ModuleTables` 半收口、第六轮已列项等）。

#### IR / 表示半废弃

- [x] **`Op::Effect` 生产路径永不构造**：已删枚举臂；walkers/inline 不再双轨匹配。
- [x] **`Block.params` 恒为空壳**：已删除字段；形参只在 `CoreFun` / `Value::Lambda.params`。
- [x] **`MapRepr::{LitMap,SmallMap}` / `SetRepr::LitSet` 名存实亡**：`repr_select` 会写出这些臂；codegen `emit_value_alloc_set` **忽略 `_repr`**，Map/Set 注释写「Always heap+finish」。与已列「空 setOf 不走 Lit」正交——是 **已选出的 Lit\* 被后端丢掉**。宜删死臂，或实现栈路径并对账。
  - **收口**：`repr_select` 不再选 LitMap/LitSet；枚举保留为 PE 标签；codegen/IR 注释对齐；空容器发 null。
- [x] **`value_ty` 仍按 LitSet/LitMap「在栈」定根，与发射分裂**：`HeapPolicy::StackLitOk` 对非空 LitSet/LitMap 当非堆；`stack.rs` 注释写 Map 但只有 List/ADT。根/逃逸启发式与真实分配不一致。宜 `may_heap` 与 codegen 共用 Lit 契约表；同步修正 `heap.rs`「栈路径」撒谎注释。
  - **收口**：Map/Set `value_alloc_may_heap` 仅非空为 true（空=null）；与 heap 发射一致。

#### 模块环 / mono·opt 内部

- [ ] **`ir.rs` ↔ `visit.rs` 包内环依赖**：`ir` 调 `visit::{max_local_in_value,rewrite_value_locals}`，`visit` 再吃 `ir` 类型。难抽 `lumia_ir`。宜 remap/`max_local` 迁入 `ir` 或独立 `ir_ops`。
- [x] **`mono/traits` ↔ `mono/specialize` 子模块环**：`directize` 已独立；`specialize`←`directize_block`，不再经 `traits`。
- [x] **`ret_ty::block_result_fixed_ty` 用空 `sum_max_arity` 建 `FunIndex`**：改为注入调用方 `FunIndex`（含 `module.sum_max_arity` + channel hint）。
- [x] **间接调用 callee 降失败时静默 `Value::Int(0)`**：见上方第六轮收口（`note_ice` + `Result`）。
- [x] **Core lower `Alt`/`With` 用 `panic!`**：见上方第六轮收口（`note_ice` + `Err`；部分 `expect` 仍留）。
- [x] **`specialize_const` 阈值三件套未收口**：见下方 RT 节收口（`lumia_abi::SPECIALIZE_CONST_*` / `INLINE_MAX_EXPAND_DEPTH`）。
- [ ] **Inline / SpecializeConst 热路径整表 `CoreFun` 深拷**：候选表 `.map(|f| (name, f.clone()))`。异于 Memo O(n²)。宜索引借用，命中再克隆 body。
- [ ] **`traits`/`specialize` 用空 `Block` `mem::replace` 抽体再改写**：为迁就 `FunIndex` 生命周期付 O(函数数) 双缓冲。宜签名切片索引或 `FunId`/arena。

#### RT / CLI / 编辑器 / 文档 / 测试

- [ ] **RT 全局初始化三轨 + 「一次缓存」双模式**：`OnceLock`（heap/sched/memo/mutator/par）vs `Once`（trap hook/pool）vs 裸 `Mutex::new`（`ADT_SHOW`/`DICTS`）；`par_worker_count` 用 `OnceLock`，`simd_f64` 用 `AtomicU8+Relaxed`。无统一 lazy/缓存惯例，亦无 Atomic Ordering 契约表。宜 `rt::globals` + 文档化 Ordering。
- [x] **锁序几乎无文档（证据加细）**：见上方第六轮收口（`lumia_rt` Lock order 文档；CI 禁令仍欠）。
- [x] **域核 `f64_elems`/`require_len` 三文件同文拷贝**：`dense_f64`/`cn_kernels`/`efe` 各一份。异于 SR `name_of` 总述——是 **RT 域 helper 未抽**。宜单一 `list::f64_view`。
  - **收口**：`list/f64_view.rs`；三文件共用。
- [x] **`f64_simd.rs` 文件级 `#![allow(dead_code)]`**：整文件放行掩盖真死码。宜按入口 `cfg`/`pub(crate)` 或 feature 门闩。
  - **收口**：去掉文件级 allow（入口均为 `pub(crate)` 且被域核调用）。
- [x] **`specialize_const` 阈值三件套未收口**：`MAX_CLONES_PER_FUN=16` / `MAX_TOTAL_CLONES=64` / `MAX_OPS=256`（另 inline `EXPAND_DEPTH`）。异于已列 `SMALL_CONTAINER` 与魔法 `0..N`。宜进 `OptOptions`/`lumia_abi` 并文档化。
  - **收口**：`lumia_abi::{SPECIALIZE_CONST_*, INLINE_MAX_EXPAND_DEPTH}`；pass 引用之。
- [x] **「nursery」名不副实**：见上方 RT/opt 节（注释已改准；bump 仍欠）。
- [x] **`check_program` ↔ `check_program_with_overlays` 近克隆**：见上方第六轮收口（`typecheck_loaded`）。
- [x] **声称「薄 CLI」，`build` 管线仍困在 `main.rs`**：已抽出 `lumia::build::{build_file,ensure_runtime_built}`（`codegen` feature）；bin 只做 CLI dispatch。
- [x] **LSP 硬编码 `auto_parallel: true`，编辑器无编译旗标面**：`initialize` + State；vscode/IDEA 设置；另支持 `workspace/didChangeConfiguration` 热更新并重分析打开缓冲。
- [ ] **编辑器门禁半边 + 版本三角分裂**：CI 仅 Linux 跑 `check_editor_assets`；脚本不管 IDEA 缩进/注释契约；vscode `0.3.9` / IDEA `0.3.0` / workspace `0.1.0` 无对账；IDEA `until-build="262.*"` 钉死单大版本。宜扩展对账脚本 + 文档「编辑器版本 ≠ 语言版本」+ 放宽/矩阵测 IDEA。
- [x] **`corosensei` 未进 `workspace.dependencies`**：见上方第六轮收口（`0.3.4`）。
- [x] **BUILD §3 对 `lumia_rt` 描述过窄 + DESIGN/BUILD「SSA/基本块」措辞**：见上方第六轮收口。
- [x] **`dump_fold_diag`：名为测试、实为 `eprintln` 转储**：见上方第六轮收口（已删）。
- [ ] **opt pass 测试密度严重不对称**：`copy_elim`/`repr_select` **0** 同文件测；`escape` 较密；`mono/mod` 仍溺测。异于第六轮「上帝模块零近距测」——是 **opt 管线内组织不对称**。宜每 pass 最低 fixture。
- [ ] **`golden_core` 对 Task/Channel 结构盲 + `crate_tests` 无 task**：≈39 个 golden 无并发 IR；`crate_tests` 仅 eq/gc/list/map_set/memo；e2e `basic.rs` ≈624 行宏海混装。坐实「四套正确性门」中 golden/RT 层缺口。宜最小 spawn/channel fixture；`crate_tests/task/`；e2e 拆 `task.rs`。

## 架构清理（已落地，详见 git 历史）

- **2026-08-16 结构收口**：`emit_adt_set_float_mask` + `lumia_abi::ADT_SET_FLOAT_MASK`；`DENSE_F64_TRAMPOLINE_SYMS` 单一真源（codegen trampoline / opt inject assert / decls 对账测）。
- **Core ABI 收口（第 1 期）**：`Value::Builtin.result_ty` + `type_at` stamp 地面 `Channel[T]`；`channel_hint` 以 stamp 为种子；dense_f64 nest matcher 仅留在 `lumia_opt`（codegen 只发 Call trampoline）；根目录 probe 由 `.gitignore` 忽略。
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

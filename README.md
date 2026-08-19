# Lumia

Rust compiler + Core IR（树形 ANF / 伪 SSA）+ LLVM 21 codegen + generational mark-sweep GC (STW minor + incremental concurrent full mark).

**Target platforms: Linux and Windows.** Repository: [TetraploidHuman/Lumia](https://github.com/TetraploidHuman/Lumia).

```bash
source scripts/env.sh          # sets LLVM_SYS_211_PREFIX + shared lib paths
cargo build -p lumia          # default: shared libLLVM (llvm-dynamic)
cargo run -p lumia -- check examples/hello.lm
cargo run -p lumia -- build examples/hello.lm -o /tmp/hello
/tmp/hello                     # prints 42
```

Windows（静态 LLVM SDK）：`cargo build -p lumia --no-default-features --features codegen`（见 [docs/BUILD.md](docs/BUILD.md) §4.2）。

Cross-platform example suite:

```bash
cargo test -p lumia --test e2e_examples
# or full local smoke (lib + e2e):
./scripts/check.sh
```

Workspace crates: `lumia` (CLI), `lumia_abi`, `lumia_syntax`, `lumia_hir`, `lumia_ty`, `lumia_core`, `lumia_opt`, `lumia_codegen`, `lumia_rt`.

- 语言设计：[docs/DESIGN.md](docs/DESIGN.md)
- **构建 / 技术栈 / 分期计划**：[docs/BUILD.md](docs/BUILD.md)（选型与编译步骤以该文档为准）
- **编辑器插件**（VS Code / IntelliJ）：[editors/README.md](editors/README.md)

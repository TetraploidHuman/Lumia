# Lumi

Rust compiler + Core SSA + LLVM 21 codegen + pluggable GC ABI (STW mark-sweep first).

**Target platforms: Linux and Windows.** Repository: [TetraploidHuman/Lumi](https://github.com/TetraploidHuman/Lumi).

```bash
source scripts/env.sh          # sets LLVM_SYS_211_PREFIX + shared lib paths
cargo build -p lumi
cargo run -p lumi -- check examples/hello.lm
cargo run -p lumi -- build examples/hello.lm -o /tmp/hello
/tmp/hello                     # prints 42
```

Cross-platform example suite:

```bash
cargo test -p lumi --test e2e_examples
# or full local smoke (lib + e2e):
./scripts/check.sh
```

Workspace crates: `lumi` (CLI), `lumi_abi`, `lumi_syntax`, `lumi_hir`, `lumi_ty`, `lumi_core`, `lumi_opt`, `lumi_codegen`, `lumi_rt`.

- 语言设计：[docs/DESIGN.md](docs/DESIGN.md)
- **构建 / 技术栈 / 分期计划**：[docs/BUILD.md](docs/BUILD.md)（选型与编译步骤以该文档为准）
- **编辑器插件**（VS Code / IntelliJ）：[editors/README.md](editors/README.md)

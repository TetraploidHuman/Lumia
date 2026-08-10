# Lumia IntelliJ IDEA plugin

Language support for Lumia (`.lm`) using the JetBrains native LSP API (`lumia lsp`).

## Features

- Syntax highlighting (hand-written lexer), braces, quotes, 4-space indent
- LSP: diagnostics, hover, go-to-definition, completion, formatting, outline, inlay hints
- Live templates (`module`, `val`, `match`, `trait`, …)
- New → Lumia File
- Run configurations: **Check** / **Build & Run**
- Tools → Lumia: Check / Build / Format / Restart Language Server
- Settings → Languages & Frameworks → Lumia (`lumia` binary path)

## Requirements

- IntelliJ IDEA **2026.2** (build 262) with the LSP module (Ultimate / unified IDE with LSP)
- `lumia` on `PATH` or configured under Settings → Lumia

```bash
source scripts/env.sh
cargo build -p lumia --release
export PATH="$PWD/target/release:$PATH"
```

Build & Run also needs the LLVM environment from `scripts/env.sh` in the IDE process environment.

## Build / install

```bash
cd editors/idea
./gradlew buildPlugin
# ZIP: build/distributions/lumia-idea-0.3.0.zip
```

Install from disk: **Settings → Plugins → ⚙ → Install Plugin from Disk…**

Debug: `./gradlew runIde`

## Note on LSP4IJ

This plugin uses the **native** JetBrains LSP client (same approach as the sibling Lumi plugin), not Red Hat LSP4IJ, so it does not require a separate LSP4IJ install. Community-only IDEs without the LSP module should use the [VS Code extension](../vscode/).

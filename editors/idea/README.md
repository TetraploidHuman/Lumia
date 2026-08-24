# Lumi IntelliJ IDEA plugin

Language support for Lumi (`.lm`) using the JetBrains native LSP API (`lumi lsp`).

## Features

- Syntax highlighting (hand-written lexer), braces, quotes, 4-space indent
- LSP: diagnostics, hover, go-to-definition, completion, formatting, outline, inlay hints
- Live templates (`module`, `val`, `match`, `trait`, …)
- **File → New → Project → Lumi** (sidebar, alongside Java/Kotlin)
- **New Lumi Project…** (Welcome / File menu alternative) — creates `Lumi.toml` + `src/main.lm` and opens the project
- New → **Lumi Project** (initialize an open empty project)
- New → Lumi File
- Run configurations: **Check** / **Build & Run**
- Tools → Lumi: Check / Build / Format / Restart Language Server
- Settings → Languages & Frameworks → Lumi (`lumi` binary path)

## Requirements

- IntelliJ IDEA **2026.2** (build 262) with the LSP module (Ultimate / unified IDE with LSP)
- `lumi` on `PATH` or configured under Settings → Lumi

```bash
source scripts/env.sh
cargo build -p lumi --release
export PATH="$PWD/target/release:$PATH"
```

Build & Run also needs the LLVM environment from `scripts/env.sh` in the IDE process environment.

## Build / install

```bash
cd editors/idea
./gradlew buildPlugin
# ZIP: build/distributions/lumi-idea-0.3.3.zip
```

Install from disk: **Settings → Plugins → ⚙ → Install Plugin from Disk…**

### JetBrains Client / Remote Development (Gateway)

Your About dialog shows **JetBrains Client** (远程开发控制器). In that mode:

1. Install the plugin **on the remote backend** while connected to a project.
   Settings → Plugins → the row must show **Remote Host** (远程主机).
2. **File → New → Project** runs on the thin client; custom **Lumi** may **not** appear in the left sidebar. This is a Remote Development limitation.
3. Use instead (on the remote workspace):
   - **New → Lumi Project**
   - **Tools → Lumi → New Lumi Project**
   - Terminal: `scripts/new_lumi_project.sh my_app ~/projects` then Gateway → Open that folder
4. For **New Project → Lumi** in the sidebar, use **local** IntelliJ IDEA Ultimate (not Gateway Client).

Debug: `./gradlew runIde`

## Note on LSP4IJ

This plugin uses the **native** JetBrains LSP client (same approach as the sibling Lumi plugin), not Red Hat LSP4IJ, so it does not require a separate LSP4IJ install. Community-only IDEs without the LSP module should use the [VS Code extension](../vscode/).

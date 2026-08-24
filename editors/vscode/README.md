# Lumi VS Code extension

Language support for Lumi (`.lm`): TextMate highlighting, snippets, and a Language Client for `lumi lsp`.

## Features

- Syntax highlighting and language configuration
- Diagnostics, hover, go-to-definition, completion, formatting, outline, inlay hints (via LSP)
- Snippets (`module`, `val`, `match`, `trait`, …)
- Commands: Run, Build & Run, Check File, Restart Language Server
- Settings: `lumi.lsp.path`, `lumi.lsp.enabled`

## Prerequisites

1. Build the Lumi CLI and put it on `PATH` (or set `lumi.lsp.path`):

```bash
source scripts/env.sh
cargo build -p lumi --release
export PATH="$PWD/target/release:$PATH"
```

2. For **Build & Run**, LLVM env from `scripts/env.sh` must be available in the integrated terminal.

## Develop / install

```bash
cd editors/vscode
npm install
npx vsce package --allow-missing-repository
cursor --install-extension lumi-0.3.5.vsix
# or: code --install-extension lumi-0.3.5.vsix
```

Also install / refresh the CLI (ships a slim `lumi-lsp` without LLVM):

```bash
./scripts/install.sh
```

After upgrading the extension, **Reload Window**. Check Output → “Lumi Language Server” for:
`[lumi] LSP command: …/lumi-lsp lsp` (should be the 3.6MB binary, not the 140MB compiler).

The extension entry is `extension.js` (no TypeScript compile step).

## Settings

| Setting | Default | Meaning |
|---------|---------|---------|
| `lumi.lsp.path` | `lumi` | Path to the `lumi` binary (`lsp` / `build` / `check`) |
| `lumi.lsp.enabled` | `true` | Enable the Lumi language server |

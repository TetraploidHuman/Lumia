# Lumia VS Code extension

Language support for Lumia (`.lm`): TextMate highlighting, snippets, and a Language Client for `lumia lsp`.

## Features

- Syntax highlighting and language configuration
- Diagnostics, hover, go-to-definition, completion, formatting, outline, inlay hints (via LSP)
- Snippets (`module`, `val`, `match`, `trait`, …)
- Commands: Run, Build & Run, Check File, Restart Language Server
- Settings: `lumia.lsp.path`, `lumia.lsp.enabled`

## Prerequisites

1. Build the Lumia CLI and put it on `PATH` (or set `lumia.lsp.path`):

```bash
source scripts/env.sh
cargo build -p lumia --release
export PATH="$PWD/target/release:$PATH"
```

2. For **Build & Run**, LLVM env from `scripts/env.sh` must be available in the integrated terminal.

## Develop / install

```bash
cd editors/vscode
npm install
```

- **Debug:** open `editors/vscode` in VS Code and press F5 (`Run Lumia Extension`).
- **Install locally:**

```bash
npx vsce package --no-dependencies
code --install-extension lumia-0.3.3.vsix
```

The extension entry is `extension.js` (no TypeScript compile step).

## Settings

| Setting | Default | Meaning |
|---------|---------|---------|
| `lumia.lsp.path` | `lumia` | Path to the `lumia` binary (`lsp` / `build` / `check`) |
| `lumia.lsp.enabled` | `true` | Enable the Lumia language server |

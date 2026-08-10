# Lumia VS Code extension

Language support for Lumia (`.lm`): TextMate highlighting, snippets, and a Language Client for `lumia lsp`.

## Features

- Syntax highlighting and language configuration
- Diagnostics, hover, go-to-definition, completion, formatting, outline (via LSP)
- Inlay hints: binding types, lambda params, call returns (via LSP)
- Snippets (`module`, `val`, `match`, `trait`, …)
- Commands: Check File, Build File, Format Document, Restart Language Server
- Tasks: `lumia: check` / `build` / `fmt`
- Settings: `lumia.path`, `lumia.lsp.trace`, `lumia.checkOnSave`

## Prerequisites

1. Build the Lumia CLI and put it on `PATH` (or set `lumia.path`):

```bash
source scripts/env.sh
cargo build -p lumia --release
export PATH="$PWD/target/release:$PATH"
```

2. For **Build File**, LLVM env from `scripts/env.sh` must be available in the integrated terminal.

## Develop / install

```bash
cd editors/vscode
npm install
npm run compile
```

- **Debug:** open `editors/vscode` in VS Code and press F5 (`Run Lumia Extension`).
- **Install locally:**

```bash
npx vsce package --no-dependencies
code --install-extension lumia-0.3.0.vsix
```

## Settings

| Setting | Default | Meaning |
|---------|---------|---------|
| `lumia.path` | `lumia` | Path to the `lumia` binary |
| `lumia.lsp.trace` | `off` | LSP message trace |
| `lumia.checkOnSave` | `false` | Extra CLI `check` on save |

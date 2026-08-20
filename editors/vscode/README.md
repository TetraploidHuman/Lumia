# Lumia VS Code extension

Language support for Lumia (`.lm`): TextMate highlighting, snippets, and a Language Client for `lumia lsp`.

## Features

- Syntax highlighting and language configuration
- Diagnostics, hover, go-to-definition, completion, formatting, outline, inlay hints (via LSP)
- Snippets (`module`, `val`, `match`, `trait`, …)
- Commands: Run, Build & Run, Check File, Restart Language Server
- Settings: `lumia.lsp.path`, `lumia.lsp.enabled`, `lumia.autoParallel`

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
npx vsce package --allow-missing-repository
cursor --install-extension lumia-0.3.9.vsix
# or: code --install-extension lumia-0.3.9.vsix
```

Also install / refresh the CLI (ships a slim `lumia-lsp` without LLVM):

```bash
./scripts/install.sh
```

After upgrading the extension, **Reload Window**. Check Output → “Lumia Language Server” for:
`[lumia] LSP command: …/lumia-lsp lsp` (should be the 3.6MB binary, not the 140MB compiler).

The extension entry is `extension.js` (no TypeScript compile step).

## Settings

| Setting | Default | Meaning |
|---------|---------|---------|
| `lumia.lsp.path` | *(empty → auto-detect)* | Path to the `lumia` / `lumia-lsp` binary |
| `lumia.lsp.enabled` | `true` | Enable the Lumia language server |
| `lumia.autoParallel` | `true` | Allow auto `List.map` / `List.fold` parallelization in LSP (like CLI; `false` ≈ `--no-parallel`) |

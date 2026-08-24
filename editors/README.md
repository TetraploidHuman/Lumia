# Lumi editor plugins

Full IDE clients for Lumi (`.lm`), backed by the compiler’s built-in language server:

```bash
lumi lsp
```

| Editor | Path | Notes |
|--------|------|--------|
| **VS Code** | [vscode/](vscode/) | TextMate + `vscode-languageclient` |
| **IntelliJ IDEA** | [idea/](idea/) | Native JetBrains LSP API (2026.2 / build 262) |
| Shared grammar / snippets | [shared/](shared/) | TextMate, language-configuration, snippets |

## Prerequisites

1. Build and install the CLI:

```bash
source scripts/env.sh
cargo build -p lumi --release
export PATH="$PWD/target/release:$PATH"
```

2. Confirm the server starts:

```bash
echo '{}' | lumi lsp   # will wait on stdin — Ctrl-C is fine after process starts
```

LSP features today: diagnostics, hover, go-to-definition, completion (`.` trigger, with type `detail`), formatting, document symbols (outline), inlay hints (bindings / params / call returns), semantic tokens (type-aware highlighting), `didClose` cleanup.

## VS Code

```bash
cd editors/vscode
npm install
npx vsce package --no-dependencies
code --install-extension lumi-0.3.3.vsix
```

Or open `editors/vscode` and press F5 to launch an Extension Development Host.

Settings: `lumi.lsp.path`, `lumi.lsp.enabled`.

## IntelliJ IDEA

Requires an IDE with the JetBrains LSP module (IntelliJ IDEA 2026.2 / Ultimate-style distribution).

```bash
cd editors/idea
./gradlew buildPlugin
# Install build/distributions/lumi-idea-0.3.0.zip via Plugins → Install from Disk
```

Configure **Settings → Languages & Frameworks → Lumi** if `lumi` is not on `PATH`.

**File → New → Project → Lumi** appears in the left sidebar (with Java/Kotlin). It creates `Lumi.toml` + `src/main.lm` using the name/location fields from the wizard.

Community-only IDEs without LSP: use the VS Code extension instead.

## Syncing shared assets

After editing [shared/](shared/), copy into the VS Code extension:

```bash
cp editors/shared/syntaxes/lumi.tmLanguage.json editors/vscode/syntaxes/
cp editors/shared/language-configuration.json editors/vscode/
cp editors/shared/snippets/lumi.json editors/vscode/snippets/
```

Or verify with `./scripts/check_editor_assets.sh` (also run from `./scripts/check.sh`).

The IntelliJ plugin uses a hand-written lexer aligned with the same keywords (see `LumiLexer.kt`).

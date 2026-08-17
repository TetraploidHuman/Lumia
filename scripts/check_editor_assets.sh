#!/usr/bin/env bash
# Fail if VS Code copies of shared editor assets drift; sanity-check keyword lists.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() {
  local shared="$1" vscode="$2"
  if ! diff -q "$shared" "$vscode" >/dev/null; then
    echo "drift: $vscode differs from $shared" >&2
    echo "  fix: cp $shared $vscode" >&2
    fail=1
  fi
}
check "$root/editors/shared/snippets/lumia.json" \
  "$root/editors/vscode/snippets/lumia.json"
check "$root/editors/shared/syntaxes/lumia.tmLanguage.json" \
  "$root/editors/vscode/syntaxes/lumia.tmLanguage.json"
check "$root/editors/shared/language-configuration.json" \
  "$root/editors/vscode/language-configuration.json"
# Duplicate JSON object keys (e.g. two "alt" snippets) — last wins silently.
python3 - "$root/editors/vscode/snippets/lumia.json" <<'PY'
import json, sys
path = sys.argv[1]
raw = open(path, encoding="utf-8").read()
# Naive duplicate-key detect: json.load keeps last; compare key counts via regex.
import re
keys = re.findall(r'^\s*"([^"]+)"\s*:\s*\{', raw, re.M)
if len(keys) != len(set(keys)):
    dup = sorted({k for k in keys if keys.count(k) > 1})
    print(f"duplicate snippet keys in {path}: {dup}", file=sys.stderr)
    sys.exit(1)
json.loads(raw)
PY
# Keyword surface: TextMate + IDEA must include lexer keywords (scope/spawn) and
# may include surface soft pure/fn. Truth source: lumia_syntax TokenKind::KEYWORDS.
python3 - "$root" <<'PY'
import pathlib, re, sys
root = pathlib.Path(sys.argv[1])
# Pull KEYWORDS array from token.rs (between KEYWORDS: … ]; before SURFACE_SOFT).
token = (root / "crates/lumia_syntax/src/token.rs").read_text(encoding="utf-8")
m = re.search(r"pub const KEYWORDS: &\[&str\] = &\[(.*?)\];", token, re.S)
if not m:
    print("could not parse TokenKind::KEYWORDS", file=sys.stderr)
    sys.exit(1)
kws = re.findall(r'"([^"]+)"', m.group(1))
must = set(kws)
tm = (root / "editors/shared/syntaxes/lumia.tmLanguage.json").read_text(encoding="utf-8")
idea = (root / "editors/idea/src/main/kotlin/org/lumia/idea/LumiaLexer.kt").read_text(
    encoding="utf-8"
)
fail = 0
for name, blob in (("tmLanguage", tm), ("IDEA LumiaLexer", idea)):
    missing = sorted(k for k in must if not re.search(rf"\b{re.escape(k)}\b", blob))
    if missing:
        print(f"{name} missing lexer keywords: {missing}", file=sys.stderr)
        fail = 1
if fail:
    sys.exit(1)
print("keyword surface: tmLanguage + IDEA cover TokenKind::KEYWORDS")
PY
# package.json version ↔ README vsix name; configuration keys documented in README.
python3 - "$root" <<'PY'
import json, pathlib, re, sys
root = pathlib.Path(sys.argv[1])
pkg = json.loads((root / "editors/vscode/package.json").read_text(encoding="utf-8"))
readme = (root / "editors/vscode/README.md").read_text(encoding="utf-8")
ver = pkg["version"]
vsix = f"lumia-{ver}.vsix"
if vsix not in readme:
    print(f"README missing packaged vsix name `{vsix}` (package.json version={ver})", file=sys.stderr)
    sys.exit(1)
props = pkg.get("contributes", {}).get("configuration", {}).get("properties", {})
missing_keys = sorted(k for k in props if f"`{k}`" not in readme and k not in readme)
if missing_keys:
    print(f"README Settings missing package.json keys: {missing_keys}", file=sys.stderr)
    sys.exit(1)
# Must keep autoParallel wired (Todo / LSP initialize options).
if "lumia.autoParallel" not in props:
    print("package.json missing lumia.autoParallel setting", file=sys.stderr)
    sys.exit(1)
print(f"vscode package {ver}: README vsix + settings keys OK")
# package-lock root version must match package.json (npm publish / install drift).
lock = json.loads((root / "editors/vscode/package-lock.json").read_text(encoding="utf-8"))
lock_ver = lock.get("version")
pkg_lock_ver = lock.get("packages", {}).get("", {}).get("version")
if lock_ver != ver or pkg_lock_ver != ver:
    print(
        f"package-lock version drift: package.json={ver} "
        f"lockfile={lock_ver} packages['']={pkg_lock_ver}",
        file=sys.stderr,
    )
    sys.exit(1)
print(f"vscode package-lock {ver}: root version OK")
# Version triangle: editor plugins ≠ Cargo workspace — document, do not force equality.
ws = (root / "Cargo.toml").read_text(encoding="utf-8")
m = re.search(r'^\[workspace\.package\]\s*\n(?:.*\n)*?version\s*=\s*"([^"]+)"', ws, re.M)
ws_ver = m.group(1) if m else "?"
idea_xml = (root / "editors/idea/src/main/resources/META-INF/plugin.xml").read_text(
    encoding="utf-8"
)
im = re.search(r"<version>([^<]+)</version>", idea_xml)
idea_ver = im.group(1) if im else "?"
um = re.search(r'until-build="([^"]+)"', idea_xml)
until = um.group(1) if um else "?"
if until == "262.*":
    print("IDEA until-build still pinned to single major 262.* — widen when smoke-tested", file=sys.stderr)
    sys.exit(1)
print(
    f"version triangle (independent): vscode={ver} IDEA={idea_ver} "
    f"(until-build={until}) workspace={ws_ver}"
)
# IDEA liveTemplates must not invent prefixes missing from shared snippets
# (third snippet surface — keys must stay a subset of editors/shared).
snippets = json.loads(
    (root / "editors/shared/snippets/lumia.json").read_text(encoding="utf-8")
)
shared_keys = set(snippets.keys())
idea_tpl = (
    root / "editors/idea/src/main/resources/liveTemplates/Lumia.xml"
).read_text(encoding="utf-8")
idea_names = set(re.findall(r'<template name="([^"]+)"', idea_tpl))
missing = sorted(idea_names - shared_keys)
if missing:
    print(
        f"IDEA liveTemplates not in shared snippets: {missing}",
        file=sys.stderr,
    )
    sys.exit(1)
print(f"IDEA liveTemplates ⊆ shared snippets ({len(idea_names)} keys)")
PY
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
echo "editor assets: shared ↔ vscode in sync"

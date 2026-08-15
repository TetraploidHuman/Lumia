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
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
echo "editor assets: shared ↔ vscode in sync"

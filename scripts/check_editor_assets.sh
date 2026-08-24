#!/usr/bin/env bash
# Fail if VS Code copies of shared editor assets drift.
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
check "$root/editors/shared/snippets/lumi.json" \
  "$root/editors/vscode/snippets/lumi.json"
check "$root/editors/shared/syntaxes/lumi.tmLanguage.json" \
  "$root/editors/vscode/syntaxes/lumi.tmLanguage.json"
check "$root/editors/shared/language-configuration.json" \
  "$root/editors/vscode/language-configuration.json"
# Duplicate JSON object keys (e.g. two "alt" snippets) — last wins silently.
python3 - "$root/editors/vscode/snippets/lumi.json" <<'PY'
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
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
echo "editor assets: shared ↔ vscode in sync"

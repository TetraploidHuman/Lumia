#!/usr/bin/env bash
# CI lint: document lock order + SchedCore Send SAFETY must stay present.
# Does not prove absence of inversions — only that the contract is still in-tree.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

lib="$root/crates/lumia_rt/src/lib.rs"
if ! grep -q 'heap → sched' "$lib"; then
  echo "lumia_rt lib.rs missing lock-order rank 'heap → sched'" >&2
  fail=1
fi
if ! grep -q 'DICTS' "$lib" || ! grep -q 'ADT_SHOW' "$lib"; then
  echo "lumia_rt lib.rs lock-order docs should mention DICTS / ADT_SHOW" >&2
  fail=1
fi

python3 - "$root/crates/lumia_rt/src/task/sched_core.rs" <<'PY'
import pathlib, sys
text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
needle = "unsafe impl Send for SchedCore"
i = text.find(needle)
if i < 0:
    print("missing unsafe impl Send for SchedCore", file=sys.stderr)
    sys.exit(1)
window = text[max(0, i - 500) : i]
if "Safety" not in window and "# Safety" not in window:
    print("SchedCore Send: missing # Safety / SAFETY in preceding 500 chars", file=sys.stderr)
    sys.exit(1)
if "home" not in window.lower():
    print("SchedCore Send SAFETY must mention home-thread invariant", file=sys.stderr)
    sys.exit(1)
print("SchedCore Send SAFETY window OK")
PY

if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
echo "rt lock-order docs + SchedCore Send SAFETY: OK"

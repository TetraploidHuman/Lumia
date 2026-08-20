#!/usr/bin/env bash
# CI lint: document lock order + SchedCore Send contract must stay present.
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
if ! grep -q 'LAB' "$lib"; then
  echo "lumia_rt lib.rs lock-order docs should mention per-mutator LAB" >&2
  fail=1
fi

# SchedCore is naturally Send after home_coro / scan_ptrs (no unsafe impl).
# Keep the home-thread invariant documented next to SchedCore, and a Send lock-in test.
python3 - "$root/crates/lumia_rt/src" <<'PY'
import pathlib, sys

src = pathlib.Path(sys.argv[1])
core = (src / "task" / "sched_core.rs").read_text(encoding="utf-8")
tests = (src / "task" / "sched_core_tests.rs").read_text(encoding="utf-8")

if "unsafe impl Send for SchedCore" in core:
    print(
        "unexpected unsafe impl Send for SchedCore "
        "(stacks are in home_coro; SchedCore should be naturally Send)",
        file=sys.stderr,
    )
    sys.exit(1)

# Doc window: comment immediately above `struct SchedBox` / SchedCore Send note.
marker = "SchedCore` is naturally `Send`"
i = core.find(marker)
if i < 0:
    # allow either backtick style
    marker = "SchedCore is naturally `Send`"
    i = core.find(marker)
if i < 0 and "naturally `Send`" not in core and "naturally Send" not in core:
    print("sched_core.rs missing natural-Send documentation for SchedCore", file=sys.stderr)
    sys.exit(1)
window = core[max(0, i - 80) : i + 500] if i >= 0 else core
if "home" not in window.lower():
    print("SchedCore Send docs must mention home-thread invariant", file=sys.stderr)
    sys.exit(1)
if "home_coro" not in window and "home-thread" not in window.lower():
    print("SchedCore Send docs must mention home_coro / home-thread", file=sys.stderr)
    sys.exit(1)

if "assert_send::<super::SchedCore>" not in tests and "assert_send::<SchedCore>" not in tests:
    print("sched_core_tests.rs missing SchedCore: Send lock-in test", file=sys.stderr)
    sys.exit(1)

print("SchedCore natural-Send docs + test OK")
PY

if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
echo "rt lock-order docs + SchedCore Send contract: OK"

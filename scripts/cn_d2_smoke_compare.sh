#!/usr/bin/env bash
# Side-by-side CogniNucleus D2 amy_motor smoke: Lumia vs Python (--lumia-align).
#
# Does not fail on numeric mismatch — the port still diverges on food/threat
# magnitudes. Prints a compact table for local diagnosis.
#
# Env:
#   COGNINUCLEUS_ROOT   default: ../CogniNucleus next to Lumia
#   COGNINUCLEUS_PYTHON default: $COGNINUCLEUS_ROOT/.venv/bin/python
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

CN_ROOT="${COGNINUCLEUS_ROOT:-$(cd "$ROOT/../CogniNucleus" && pwd)}"
CN_PYTHON="${COGNINUCLEUS_PYTHON:-$CN_ROOT/.venv/bin/python}"
OUT_DIR="${TMPDIR:-/tmp}/lumia_cn_d2_compare"
mkdir -p "$OUT_DIR"

if [[ ! -x "$CN_PYTHON" ]]; then
  echo "ERROR: Python not found: $CN_PYTHON" >&2
  exit 1
fi
if [[ ! -f "$CN_ROOT/eval_behaviors.py" ]]; then
  echo "ERROR: eval_behaviors.py missing under $CN_ROOT" >&2
  exit 1
fi

echo "== Lumia gate =="
"$ROOT/scripts/cn_d2_smoke_gate.sh" | tee "$OUT_DIR/lumia.txt"

echo "== Python --smoke --lumia-align =="
(
  cd "$CN_ROOT"
  PYTHONPATH=. "$CN_PYTHON" eval_behaviors.py \
    --smoke --stage D2 --ablation amy_motor --lumia-align
) | tee "$OUT_DIR/python.txt"

python3 - "$OUT_DIR/lumia.txt" "$OUT_DIR/python.txt" <<'PY'
import json, re, sys
from pathlib import Path

def lumia_kv(text: str) -> dict:
    lines = text.splitlines()
    kv = {}
    for i in range(len(lines) - 1):
        k, v = lines[i], lines[i + 1]
        if k and (" " not in k) and not k.startswith("=") and not k.startswith("wall"):
            kv[k] = v
    return kv

def py_metrics(text: str) -> dict:
    # Summary JSON is the last {...} block
    start = text.rfind("{")
    if start < 0:
        raise SystemExit("no JSON summary in Python output")
    # find matching end by brute brace depth from last top-level object start
    # Prefer: locate '"mean_pass_rate"' vicinity — parse from first { after last seed header
    m = re.search(r"\{\s*\"stage\":\s*\"D2\".*\}\s*$", text, re.S)
    if not m:
        # fall back: from last line that is just "{"
        idx = text.rfind('\n{\n')
        blob = text[idx + 1 :] if idx >= 0 else text[start:]
    else:
        blob = m.group(0)
    # trim to balanced JSON
    depth = 0
    end = None
    for i, ch in enumerate(blob):
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
            if depth == 0:
                end = i + 1
                break
    if end is None:
        raise SystemExit("unbalanced JSON in Python output")
    data = json.loads(blob[:end])
    r0 = data["results"][0]
    abl = r0.get("ablation") or {}
    return {
        "pass_rate": r0["pass_rate"],
        "B1": int(r0["behaviors"]["B1_foraging"]["pass"]),
        "B2": int(r0["behaviors"]["B2_threat_avoid"]["pass"]),
        "B3": int(r0["behaviors"]["B3_homeostasis_explore"]["pass"]),
        "B8": int(bool(abl.get("B8_amy_lesion_worse"))),
        "agent_mean_threat": r0["agent"]["mean_threat_contacts"],
        "random_mean_threat": r0["random"]["mean_threat_contacts"],
        "agent_mean_food": r0["agent"]["mean_food_visits"],
        "random_mean_food": r0["random"]["mean_food_visits"],
        "ablation_mean_threat": (abl.get("agent") or {}).get("mean_threat_contacts"),
    }

lk = lumia_kv(Path(sys.argv[1]).read_text())
pk = py_metrics(Path(sys.argv[2]).read_text())

rows = [
    ("pass_rate", float(lk["pass_rate"]), float(pk["pass_rate"])),
    ("B1", int(lk["B1"]), int(pk["B1"])),
    ("B2", int(lk["B2"]), int(pk["B2"])),
    ("B3", int(lk["B3"]), int(pk["B3"])),
    ("B8", int(lk["B8_amy_lesion_worse"]), int(pk["B8"])),
    ("agent_threat", float(lk["agent_mean_threat"]), float(pk["agent_mean_threat"])),
    ("random_threat", float(lk["random_mean_threat"]), float(pk["random_mean_threat"])),
    ("agent_food", float(lk["agent_mean_food"]), float(pk["agent_mean_food"])),
    ("random_food", float(lk["random_mean_food"]), float(pk["random_mean_food"])),
]

print()
print(f"{'metric':<16} {'lumia':>12} {'python':>12} {'match':>6}")
for name, lv, pv in rows:
    match = "yes" if lv == pv else "no"
    if isinstance(lv, float):
        print(f"{name:<16} {lv:12.4f} {pv:12.4f} {match:>6}")
    else:
        print(f"{name:<16} {lv:12d} {pv:12d} {match:>6}")
print()
print("note: numeric mismatch is expected until env/agent RNG+init fully match Python --lumia-align")
PY

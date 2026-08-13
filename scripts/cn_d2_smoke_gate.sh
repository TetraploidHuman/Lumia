#!/usr/bin/env bash
# Gate CogniNucleusForLumia D2 amy_motor smoke (Release).
#
# Builds eval_behaviors_smoke.lm, runs it, and checks behavioral invariants:
#   metrics_sane=1, threat metrics finite, B8 consistent with delta,
#   B2 when random_threat>0 (else both threats ~0).
# Wall time is reported but not gated (machine noise).
#
# Full numeric parity with Python `eval_behaviors.py --smoke --lumia-align` is
# not required yet (post-train LCG streams still diverge). Use
# `scripts/cn_d2_smoke_compare.sh` for a side-by-side dump.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

cd "$ROOT"
# Prefer a prebuilt compiler (CI sets LUMIA + builds with llvm-dynamic).
if [[ -n "${LUMIA:-}" && -x "$LUMIA" ]]; then
  echo "== using LUMIA=$LUMIA =="
else
  # Optional: LUMIA_CARGO_FEATURES='--features llvm-dynamic' on apt.llvm.org hosts.
  # shellcheck disable=SC2086
  cargo build -q -p lumia --release ${LUMIA_CARGO_FEATURES:-}
  LUMIA="$ROOT/target/release/lumia"
fi
OUT_DIR="${TMPDIR:-/tmp}/lumia_cn_d2_smoke"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/eval_behaviors_smoke"
LOG="$OUT_DIR/out.txt"

echo "== build =="
"$LUMIA" build --release CogniNucleusForLumia/eval_behaviors_smoke.lm -o "$BIN"

echo "== run =="
TIMEFORMAT='wall_sec=%R'
{
  time "$BIN" >"$LOG"
} 2>"$OUT_DIR/time.txt"
cat "$OUT_DIR/time.txt"
tail -n 30 "$LOG"

python3 - "$LOG" <<'PY'
import sys
from pathlib import Path

lines = Path(sys.argv[1]).read_text().splitlines()
kv = {}
i = 0
while i + 1 < len(lines):
    k, v = lines[i], lines[i + 1]
    if k and (" " not in k) and not k.startswith("="):
        kv[k] = v
    i += 1

def need(key: str) -> str:
    if key not in kv:
        raise SystemExit(f"missing metric {key!r}")
    return kv[key]

sane = need("metrics_sane")
b1, b2, b3 = need("B1"), need("B2"), need("B3")
b8 = need("B8_amy_lesion_worse")
pass_rate = float(need("pass_rate"))
agent_threat = float(need("agent_mean_threat"))
random_threat = float(need("random_mean_threat"))
ablation_threat = float(need("ablation_mean_threat"))
b2_delta = float(need("B2_threat_delta"))

if sane != "1":
    raise SystemExit(f"metrics_sane={sane} (want 1)")
if b1 not in ("0", "1") or b2 not in ("0", "1") or b3 not in ("0", "1") or b8 not in ("0", "1"):
    raise SystemExit(f"behavior bits must be 0|1, got B1={b1} B2={b2} B3={b3} B8={b8}")
if not (0.0 <= pass_rate <= 1.0):
    raise SystemExit(f"pass_rate={pass_rate} out of range")
if agent_threat > 1e6 or random_threat > 1e6 or ablation_threat > 1e6:
    raise SystemExit("threat metric looks like Float-as-Int corruption")
# B2: agent avoids threats relative to random. When the post-train LCG stream
# yields no threat contacts for either policy, the ratio test is undefined —
# require both stay at ~0 (no agent "inventing" contacts).
if random_threat > 1e-9:
    if b2 != "1":
        raise SystemExit(f"B2={b2} (want 1; agent should avoid threats vs random)")
    if not (agent_threat < random_threat):
        raise SystemExit(
            f"agent_mean_threat={agent_threat} not < random_mean_threat={random_threat}"
        )
elif agent_threat > 1e-9:
    raise SystemExit(
        f"random_mean_threat≈0 but agent_mean_threat={agent_threat}"
    )
# B8 definition in smoke: lesion raises threat by > 0.5
if b8 == "1" and b2_delta <= 0.5:
    raise SystemExit(f"B8=1 but B2_threat_delta={b2_delta} <= 0.5")
if b8 == "0" and b2_delta > 0.5:
    raise SystemExit(f"B8=0 but B2_threat_delta={b2_delta} > 0.5")
# B1 foraging signal should remain defined (agent beats or trails random on food).
agent_food = float(need("agent_mean_food"))
random_food = float(need("random_mean_food"))
if agent_food > 1e6 or random_food > 1e6:
    raise SystemExit("food metric looks like Float-as-Int corruption")

print(
    "OK",
    f"B1={b1}",
    f"B2={b2}",
    f"B3={b3}",
    f"B8={b8}",
    f"pass_rate={pass_rate}",
    f"agent_threat={agent_threat}",
    f"random_threat={random_threat}",
    f"agent_food={agent_food}",
)
PY

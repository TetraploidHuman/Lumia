#!/usr/bin/env bash
# Gate CogniNucleusForLumia D2 amy_motor smoke (Release).
#
# Builds eval_behaviors_smoke.lm, runs it, and checks key behavioral bits
# stay sane (metrics_sane, B2 threat avoidance). Wall time is reported but
# not gated (machine noise).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"

cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT_DIR="${TMPDIR:-/tmp}/lumia_cn_d2_smoke"
mkdir -p "$OUT_DIR"
BIN="$OUT_DIR/eval_behaviors_smoke"
LOG="$OUT_DIR/out.txt"

echo "== build =="
"$LUMIA" build --release CogniNucleusForLumia/eval_behaviors_smoke.lm -o "$BIN"

echo "== run =="
# /usr/bin/time may be missing on Nix; use bash TIMEFORMAT.
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
    # metric keys are single tokens without spaces
    if k and (" " not in k) and not k.startswith("="):
        kv[k] = v
    i += 1

def need(key: str) -> str:
    if key not in kv:
        raise SystemExit(f"missing metric {key!r}")
    return kv[key]

sane = need("metrics_sane")
b2 = need("B2")
pass_rate = float(need("pass_rate"))
threat = float(need("agent_mean_threat"))

if sane != "1":
    raise SystemExit(f"metrics_sane={sane} (want 1)")
if b2 != "1":
    raise SystemExit(f"B2={b2} (want 1; agent should avoid threats vs random)")
if not (0.0 <= pass_rate <= 1.0):
    raise SystemExit(f"pass_rate={pass_rate} out of range")
if threat > 1e6:
    raise SystemExit(f"agent_mean_threat={threat} looks like Float-as-Int corruption")

print("OK", f"B2={b2}", f"pass_rate={pass_rate}", f"agent_mean_threat={threat}")
PY

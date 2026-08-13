#!/usr/bin/env bash
# Build & run CogniNucleus-For-Lumia demo (Release).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$ROOT/scripts/env.sh"
cd "$ROOT"
cargo build -q -p lumia --release
LUMIA="$ROOT/target/release/lumia"
OUT="${TMPDIR:-/tmp}/cogninucleus_for_lumia"
mkdir -p "$OUT"
"$LUMIA" build --release CogniNucleusForLumia/main.lm -o "$OUT/demo"
echo "== run =="
"$OUT/demo"
echo "OK"

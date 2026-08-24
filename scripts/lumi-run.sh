#!/usr/bin/env bash
# Run `lumi` with NixOS shared-library paths (for IDEA / GUI launchers).
set -euo pipefail
SCRIPT="${BASH_SOURCE[0]}"
ROOT="$(cd "${SCRIPT%/*}/.." && pwd)"
# shellcheck source=scripts/env.sh
source "$ROOT/scripts/env.sh" >/dev/null 2>&1 || true
LUMI="${LUMI_BIN:-$ROOT/target/release/lumi}"
if [[ ! -x "$LUMI" ]]; then
  LUMI="$(command -v lumi 2>/dev/null || true)"
fi
if [[ -z "$LUMI" || ! -x "$LUMI" ]]; then
  echo "lumi not found (build: source scripts/env.sh && cargo build -p lumi --release)" >&2
  exit 127
fi
exec "$LUMI" "$@"

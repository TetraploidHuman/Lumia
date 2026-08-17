#!/usr/bin/env bash
# Remove ad-hoc probe binaries and .o files left in the repo root
# (gitignore keeps them out of git, but they still clutter `ls` / disk).
#
# Linux-only: probe detection is ELF magic (`0x7fELF`). Windows PE leftovers
# are not cleaned here.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

is_probe_binary() {
  local f="$1"
  [[ -f "$f" && -x "$f" ]] || return 1
  # ELF: 0x7f E L F — no dependency on `file(1)`.
  local mag
  mag=$(od -An -N4 -tx1 "$f" 2>/dev/null | tr -d ' \n') || return 1
  [[ "$mag" == "7f454c46" ]] || return 1
  return 0
}

removed=0
shopt -s nullglob
for f in ./*.o; do
  rm -f -- "$f"
  removed=$((removed + 1))
done

for f in ./*; do
  [[ -f "$f" ]] || continue
  case "$(basename "$f")" in
    *.sh|*.ps1|*.md|*.toml|*.lock|*.txt|LICENSE|Todo.md) continue ;;
  esac
  if is_probe_binary "$f"; then
    rm -f -- "$f"
    removed=$((removed + 1))
  fi
done

echo "clean_probes: removed $removed root probe artifact(s)"

#!/usr/bin/env bash
# Source this before building/running Lumi on NixOS.
#   source scripts/env.sh

if [[ -z "${LLVM_SYS_211_PREFIX:-}" ]]; then
  for p in /nix/store/*-llvm-21.1.8-dev; do
    if [[ -x "$p/bin/llvm-config" ]]; then
      export LLVM_SYS_211_PREFIX="$p"
      break
    fi
  done
fi

if [[ -n "${LLVM_SYS_211_PREFIX:-}" ]]; then
  export PATH="$LLVM_SYS_211_PREFIX/bin:$PATH"
fi

export PATH="${HOME}/.cargo/bin:$PATH"

# Collect shared library dirs only — never *-static (incompatible with rust-lld).
_LIB_DIRS=()
_add_lib() {
  local p="$1"
  if [[ -d "$p" && "$p" != *"-static"* && "$p" != *static/lib ]]; then
    _LIB_DIRS+=("$p")
  fi
}

for p in \
  /nix/store/*-llvm-21.1.8-lib/lib \
  /nix/store/*-libffi-*/lib \
  /nix/store/*-zlib-*/lib \
  /nix/store/*-libxml2-*/lib \
  /nix/store/*-gcc-*-lib/lib; do
  _add_lib "$p"
done

# Drop any previously injected static zlib paths.
_filter_path() {
  local in="$1" out="" part
  IFS=':' read -ra parts <<< "$in"
  for part in "${parts[@]}"; do
    [[ -z "$part" ]] && continue
    [[ "$part" == *"-static"* ]] && continue
    [[ "$part" == *static/lib ]] && continue
    if [[ -z "$out" ]]; then
      out="$part"
    else
      out="$out:$part"
    fi
  done
  echo "$out"
}

_JOINED=$(IFS=:; echo "${_LIB_DIRS[*]}")
# Defaults first so `set -u` (e.g. scripts/check.sh) never trips on unset paths.
: "${LIBRARY_PATH:=}"
: "${LD_LIBRARY_PATH:=}"
if [[ -n "$LIBRARY_PATH" ]]; then
  export LIBRARY_PATH="$(_filter_path "${_JOINED}:$LIBRARY_PATH")"
else
  export LIBRARY_PATH="$(_filter_path "$_JOINED")"
fi
if [[ -n "$LD_LIBRARY_PATH" ]]; then
  export LD_LIBRARY_PATH="$(_filter_path "${_JOINED}:$LD_LIBRARY_PATH")"
else
  export LD_LIBRARY_PATH="$(_filter_path "$_JOINED")"
fi

echo "Lumi env: LLVM_SYS_211_PREFIX=${LLVM_SYS_211_PREFIX:-unset}"

#!/usr/bin/env bash
# Create a minimal Lumia package on disk (remote-friendly; no IDE required).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

usage() {
  echo "Usage: $0 NAME [PARENT_DIR]" >&2
  echo "  Creates PARENT_DIR/NAME/{Lumia.toml,src/main.lm} (default PARENT_DIR: \$PWD)" >&2
  exit 1
}

[[ $# -ge 1 ]] || usage
NAME="$1"
PARENT="${2:-$PWD}"
DIR="$PARENT/$NAME"

if [[ ! "$NAME" =~ ^[A-Za-z0-9_-]+$ ]]; then
  echo "error: package name must be [A-Za-z0-9_-]+ (got '$NAME')" >&2
  exit 1
fi
if [[ -e "$DIR" ]]; then
  echo "error: already exists: $DIR" >&2
  exit 1
fi

MODULE="${NAME//-/_}"
mkdir -p "$DIR/src"
cat >"$DIR/Lumia.toml" <<EOF
[package]
name = "$NAME"
version = "0.1.0"
EOF
cat >"$DIR/src/main.lm" <<EOF
module $MODULE

import std.io.{println}

val main = {
    println(42)
}
EOF

echo "created $DIR"
echo "  Lumia.toml"
echo "  src/main.lm"
echo
echo "Open in IDEA (remote): JetBrains Gateway → Open → $DIR"

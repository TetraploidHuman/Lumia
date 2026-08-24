#!/usr/bin/env python3
"""Convert top-level `val name = { params -> body }` to `val name(params) = { body }`."""
from __future__ import annotations

import re
import sys
from pathlib import Path

# Top-level only: start of line (optional indent for nested files is ok if no leading spaces for module items)
HEAD = re.compile(
    r"^(?P<prefix>(?:priv\s+)?)val\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*=\s*\{\s*"
    r"(?P<params>[A-Za-z_][A-Za-z0-9_]*(?:\s*:\s*[A-Za-z_][A-Za-z0-9_]*)?"
    r"(?:\s*,\s*[A-Za-z_][A-Za-z0-9_]*(?:\s*:\s*[A-Za-z_][A-Za-z0-9_]*)?)*)"
    r"\s*->\s*(?P<rest>.*)$"
)


def convert_text(src: str) -> tuple[str, int]:
    lines = src.splitlines(keepends=True)
    out: list[str] = []
    i = 0
    n = 0
    while i < len(lines):
        line = lines[i]
        # Only convert at column 0 (module-level vals)
        if line.startswith("val ") or line.startswith("priv val "):
            m = HEAD.match(line.rstrip("\n").rstrip("\r"))
            if m:
                nl = "\n" if line.endswith("\n") else ""
                prefix = m.group("prefix") or ""
                name = m.group("name")
                params = re.sub(r"\s+", " ", m.group("params").strip())
                rest = m.group("rest")
                if rest.strip() == "":
                    # Multi-line: `val f = { a, b ->`  then body lines until matching `}`
                    out.append(f"{prefix}val {name}({params}) = {{{nl}")
                    n += 1
                    i += 1
                    continue
                # Single-line body after ->  (rest includes closing `}`)
                body = rest.strip()
                if body.endswith("}"):
                    body = body[:-1].rstrip()
                out.append(f"{prefix}val {name}({params}) = {{ {body} }}{nl}")
                n += 1
                i += 1
                continue
        out.append(line)
        i += 1
    return "".join(out), n


def main() -> int:
    roots = [Path(p) for p in sys.argv[1:]] or [Path("std")]
    total = 0
    for root in roots:
        files = sorted(root.rglob("*.lm")) if root.is_dir() else [root]
        for path in files:
            text = path.read_text(encoding="utf-8")
            new, count = convert_text(text)
            if count:
                path.write_text(new, encoding="utf-8")
                print(f"{path}: {count}")
                total += count
    print(f"total conversions: {total}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

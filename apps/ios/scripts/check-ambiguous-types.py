#!/usr/bin/env python3
"""Fail when a Swift file imports both CapstanAppCore and CapstanKit and uses, unqualified, a public type name that
both modules define (Xcode reports such a name as "ambiguous for type lookup").

Usage: check-ambiguous-types.py ROOT   (ROOT is the repository root)
Prints "path:line: [ambiguous] ..." per finding and exits 1 when there is one."""
import re
import sys
from pathlib import Path

DECL = re.compile(r"^\s*(?:@\w+\s+)*(?:public|open)\s+(?:final\s+)?(?:indirect\s+)?"
                  r"(?:struct|class|enum|protocol|actor|typealias)\s+([A-Za-z_]\w*)", re.M)
IMPORT = re.compile(r"^\s*(?:@\w+\s+)*import\s+(\w+)", re.M)
OWN = (("apps", "macos", "CapstanKit"), ("apps", "macos", "CapstanAppCore"))


def public_names(root: Path, module: str) -> set:
    found = set()
    for f in (root / "apps/macos" / module / "Sources").rglob("*.swift"):
        found.update(DECL.findall(f.read_text(encoding="utf-8", errors="replace")))
    return found


def main() -> int:
    root = Path(sys.argv[1])
    clash = public_names(root, "CapstanKit") & public_names(root, "CapstanAppCore")
    if not clash:
        return 0
    # an unqualified use: not preceded by "." or a name character, not followed by a name character
    use = re.compile(r"(?<![\w.])(" + "|".join(sorted(map(re.escape, clash))) + r")(?!\w)")
    bad = 0
    for f in sorted((root / "apps").rglob("*.swift")):
        rel = f.relative_to(root).parts
        if rel[:3] in OWN or ".build" in rel:
            continue  # the modules' own sources see only their own name
        text = f.read_text(encoding="utf-8", errors="replace")
        if not {"CapstanKit", "CapstanAppCore"} <= set(IMPORT.findall(text)):
            continue
        for n, line in enumerate(text.splitlines(), 1):
            m = use.search(line.split("//", 1)[0])
            if m:
                print(f"{'/'.join(rel)}:{n}: [ambiguous] unqualified {m.group(1)}, defined in both CapstanKit and "
                      f"CapstanAppCore: {line.strip()}")
                bad = 1
    return bad


sys.exit(main())

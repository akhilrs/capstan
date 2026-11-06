#!/usr/bin/env python3
"""Checks that ApproveStrings.swift holds the fixed strings of the approve design, key for key and value for value.

  check-approve-strings.py [repository-root]

Compares docs/design/plan-153-design-approve/strings.json with
apps/macos/CapstanAppCore/Sources/CapstanAppCore/Inbox/ApproveStrings.swift. A key `a.b_c` is the Swift name
`aBC`; each is declared as `public static let <name> = "<value>"`. Prints one finding per line and exits 1 on a
missing, extra or different string. Standard library only; run as python3 -I.
"""
import json
import os
import re
import sys

STRINGS = "docs/design/plan-153-design-approve/strings.json"
SWIFT = "apps/macos/CapstanAppCore/Sources/CapstanAppCore/Inbox/ApproveStrings.swift"
DECL = re.compile(r'^\s*public static let ([A-Za-z0-9_]+) = "((?:[^"\\]|\\.)*)"\s*$')


def swift_name(key):
    parts = re.split(r"[._]", key)
    return parts[0] + "".join(p[:1].upper() + p[1:] for p in parts[1:])


def swift_literal(text):
    """The string as the Swift literal that holds it."""
    return text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")


def main(argv):
    here = os.path.dirname(os.path.abspath(__file__))
    root = argv[1] if len(argv) > 1 else os.path.abspath(os.path.join(here, "..", "..", ".."))
    try:
        with open(os.path.join(root, STRINGS), encoding="utf-8") as f:
            design = json.load(f)
        with open(os.path.join(root, SWIFT), encoding="utf-8") as f:
            source = f.read().splitlines()
    except (OSError, ValueError) as e:
        print(f"cannot read the inputs: {e}")
        return 1
    declared = {}
    for line in source:
        m = DECL.match(line)
        if m:
            declared[m.group(1)] = m.group(2)
    problems = []
    expected = {}
    for key, value in design.items():
        name = swift_name(key)
        if name in expected:
            problems.append(f"{STRINGS}: keys collide on {name}")
        expected[name] = (key, swift_literal(value))
    for name, (key, literal) in expected.items():
        if name not in declared:
            problems.append(f"{SWIFT}: missing {name} for {key}")
        elif declared[name] != literal:
            problems.append(f"{SWIFT}: {name} differs from {key}")
    for name in declared:
        if name not in expected:
            problems.append(f"{SWIFT}: {name} is not a key of strings.json")
    for p in problems:
        print(p)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

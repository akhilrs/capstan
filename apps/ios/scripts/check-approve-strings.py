#!/usr/bin/env python3
"""Fails when ApproveStrings.swift and the approved design's strings.json differ in a key or a value.

  check-approve-strings.py [repository-root]

Each constant of ApproveStrings.swift is one line ending in `// key: <key>`; its value is a string literal that is read
as JSON (so it may use only the escapes JSON and Swift share). Standard library only.
"""
import json
import os
import re
import sys

LINE = re.compile(r'^\s*public static let (\w+) = (".*") // key: ([\w.]+)\s*$')


def main(argv):
    root = argv[1] if len(argv) > 1 else os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
    design = os.path.join(root, "docs/design/plan-153-design-approve/strings.json")
    swift = os.path.join(root, "apps/ios/CapstanPhoneCore/Sources/CapstanPhoneCore/Approve/ApproveStrings.swift")
    with open(design, encoding="utf-8") as handle:
        expected = json.load(handle)
    found, problems, names = {}, [], {}
    with open(swift, encoding="utf-8") as handle:
        for number, line in enumerate(handle, 1):
            if "public static let" not in line:
                continue
            match = LINE.match(line.rstrip("\n"))
            if not match:
                problems.append(f"{swift}:{number}: not a constant of the form `public static let name = \"value\" // key: a.b`")
                continue
            name, literal, key = match.groups()
            try:
                value = json.loads(literal)
            except ValueError:
                problems.append(f"{swift}:{number}: the value of {key} is not a plain string literal")
                continue
            if key in found:
                problems.append(f"{swift}:{number}: key {key} appears twice")
            if name in names:
                problems.append(f"{swift}:{number}: constant {name} appears twice")
            found[key], names[name] = value, key
    for key in expected:
        if key not in found:
            problems.append(f"missing key {key}")
        elif found[key] != expected[key]:
            problems.append(f"value of {key} differs:\n  design: {expected[key]!r}\n  swift:  {found[key]!r}")
    for key in found:
        if key not in expected:
            problems.append(f"key {key} is not in strings.json")
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    print(f"approve strings ok: {len(found)} keys")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

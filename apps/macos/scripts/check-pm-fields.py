#!/usr/bin/env python3
"""Checks that the PM fields the Swift readers decode are documented and covered by the kernel fixtures.

  check-pm-fields.py [repository-root]

Reads, in apps/macos/CapstanKit/Sources/CapstanKit:
  Protocol/Items.swift   the names decoded for PmCard (and Waiting, FromUser, Stuck)
  Protocol/Events.swift  the names decoded for the pm.stuck and pm.recovered bodies
and requires each name to appear in docs/remote-protocol.md; a name of an event body must also appear in
rust/crates/kernel/tests/rust_only/fixtures/event_feed/<event>.json. Prints "file: name ..." and exits 1 when
a name is missing, or when a reader that must hold names holds none. Standard library only; run as python3 -I.
"""
import os
import re
import sys

KIT = "apps/macos/CapstanKit/Sources/CapstanKit"
DOC = "docs/remote-protocol.md"
FIXTURES = "rust/crates/kernel/tests/rust_only/fixtures/event_feed"

# A member read from the wire: json.wireString("x"), raw.string("x"), json.optional("x"), ...
READ = re.compile(r'\.(?:wire[A-Za-z0-9]*|string|optionalString|int64|optionalInt64|bool|optionalBool|optional|required)\(\s*"([A-Za-z0-9_]+)"')


def read(root, rel):
    with open(os.path.join(root, rel), encoding="utf-8") as handle:
        return handle.read()


def names(source, start, end):
    """The member names read between the first `start` and the next `end` (or the end of the file)."""
    begin = source.find(start)
    if begin < 0:
        return None
    stop = source.find(end, begin + len(start)) if end else -1
    block = source[begin:stop if stop >= 0 else len(source)]
    return sorted(set(READ.findall(block)))


def main(argv):
    here = os.path.dirname(os.path.abspath(__file__))
    root = argv[1] if len(argv) > 1 else os.path.abspath(os.path.join(here, "..", "..", ".."))
    problems = []

    def missing_file(rel):
        problems.append(f"{rel}: file not found")

    try:
        doc = read(root, DOC)
    except OSError:
        missing_file(DOC)
        doc = ""

    def in_doc(name):
        return f"`{name}`" in doc or f'"{name}"' in doc

    items_rel, events_rel = f"{KIT}/Protocol/Items.swift", f"{KIT}/Protocol/Events.swift"
    try:
        items = read(root, items_rel)
    except OSError:
        missing_file(items_rel)
        items = ""
    try:
        events = read(root, events_rel)
    except OSError:
        missing_file(events_rel)
        events = ""

    card = names(items, "public struct PmCard:", "/// A message from an agent to the person")
    if not card:
        problems.append(f"{items_rel}: no PmCard field names found")
    for name in card or []:
        if not in_doc(name):
            problems.append(f"{DOC}: {name} (decoded for PmCard in {items_rel})")

    for event, reader in (("pm.stuck", "public var pmStuckBody"), ("pm.recovered", "public var pmRecoveredBody")):
        found = names(events, reader, "public var ")
        if not found:
            problems.append(f"{events_rel}: no field names found for {event}")
            continue
        fixture_rel = f"{FIXTURES}/{event}.json"
        try:
            fixture = read(root, fixture_rel)
        except OSError:
            missing_file(fixture_rel)
            fixture = ""
        for name in found:
            if not in_doc(name):
                problems.append(f"{DOC}: {name} (decoded for {event} in {events_rel})")
            if fixture and f'"{name}"' not in fixture:
                problems.append(f"{fixture_rel}: {name}")

    if problems:
        for line in problems:
            print(line, file=sys.stderr)
        return 1
    print("check-pm-fields: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

#!/usr/bin/env python3
"""Shared-code platform guard for the iPhone app. A guard, not a proof: only an iOS build shows the code compiles.

  check-shared-platform.py [repo root]   check the tree (default: the repository this script lives in)
  check-shared-platform.py --self-test   plant each pattern in a temporary copy and expect a finding (or none)

Rules:
  - CapstanKit and CapstanAppCore sources, outside `#if os(macOS)` and comments, use none of `Process(`, `kill(`,
    `NSWorkspace`, `NSSound`, `AppKit` or `/usr/bin/` (except the `sshPath` constant in RemoteCommand.swift).
  - CapstanPhoneCore sources import only Foundation, CryptoKit, Security, LocalAuthentication, CapstanKit and
    CapstanAppCore.

Exit 0 pass, 1 finding (each printed as file:line: [rule] text), 2 usage.
"""
import os
import re
import shutil
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_ROOT = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
SHARED = ("apps/macos/CapstanKit/Sources", "apps/macos/CapstanAppCore/Sources")
PHONE_CORE = "apps/ios/CapstanPhoneCore/Sources"
PHONE_IMPORTS = {"Foundation", "CryptoKit", "Security", "LocalAuthentication", "CapstanKit", "CapstanAppCore"}

TOKENS = [
    ("Process", re.compile(r"(?<![A-Za-z0-9_])Process\(")),
    ("kill", re.compile(r"(?<![A-Za-z0-9_])kill\(")),
    ("NSWorkspace", re.compile(r"(?<![A-Za-z0-9_])NSWorkspace(?![A-Za-z0-9_])")),
    ("NSSound", re.compile(r"(?<![A-Za-z0-9_])NSSound(?![A-Za-z0-9_])")),
    ("AppKit", re.compile(r"(?<![A-Za-z0-9_])AppKit(?![A-Za-z0-9_])")),
    ("usr-bin", re.compile(r"/usr/bin/")),
]
SSH_PATH = re.compile(r'\bstatic\s+let\s+sshPath\s*=\s*"/usr/bin/ssh"')
MACOS = re.compile(r"^os\(\s*macOS\s*\)$")
DIRECTIVE = re.compile(r"^\s*#\s*(if|elseif|else|endif)\b(.*)$")
IMPORT = re.compile(r"^\s*(?:@[A-Za-z_]+\s+)*import\s+(?:(?:typealias|struct|class|enum|protocol|let|var|func)\s+)?([A-Za-z_][A-Za-z0-9_]*)")


def code_lines(text):
    """The text line by line with comments removed; string contents are kept."""
    out, line = [], []
    i, n = 0, len(text)
    depth = 0          # block comment nesting
    string = None      # None, '"', '"""', or a raw-string closer like '"#'
    while i < n:
        c = text[i]
        if c == "\n":
            out.append("".join(line))
            line = []
            i += 1
            continue
        if depth:
            if text.startswith("/*", i):
                depth += 1
                i += 2
            elif text.startswith("*/", i):
                depth -= 1
                i += 2
            else:
                i += 1
            continue
        if string:
            if string == '"' and c == "\\":
                line.append(text[i : i + 2])
                i += 2
                continue
            if text.startswith(string, i):
                line.append(string)
                i += len(string)
                string = None
                continue
            line.append(c)
            i += 1
            continue
        if text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
            continue
        if text.startswith("/*", i):
            depth = 1
            i += 2
            continue
        m = re.match(r'(#+)"', text[i:])
        if m:
            string = '"' + m.group(1)
            line.append(m.group(0))
            i += len(m.group(0))
            continue
        if text.startswith('"""', i):
            string = '"""'
            line.append('"""')
            i += 3
            continue
        if c == '"':
            string = '"'
            line.append(c)
            i += 1
            continue
        line.append(c)
        i += 1
    out.append("".join(line))
    return out


def swift_files(base):
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames.sort()
        for name in sorted(filenames):
            if name.endswith(".swift"):
                yield os.path.join(dirpath, name)


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def check_shared_file(path, rel):
    findings = []
    # Each entry: (macOS only from the enclosing blocks, macOS only in this branch).
    stack = []
    for number, line in enumerate(code_lines(read(path)), start=1):
        m = DIRECTIVE.match(line)
        if m:
            word, cond = m.group(1), m.group(2).strip()
            outer = stack[-1][0] or stack[-1][1] if stack else False
            if word == "if":
                stack.append((outer, bool(MACOS.match(cond))))
            elif word in ("elseif", "else"):
                if stack:
                    parent = stack[-1][0]
                    stack[-1] = (parent, word == "elseif" and bool(MACOS.match(cond)))
            elif stack:
                stack.pop()
            continue
        if stack and (stack[-1][0] or stack[-1][1]):
            continue
        for rule, pattern in TOKENS:
            if not pattern.search(line):
                continue
            if rule == "usr-bin" and os.path.basename(path) == "RemoteCommand.swift" and SSH_PATH.search(line) \
                    and len(pattern.findall(line)) == 1:
                continue
            findings.append(f"{rel}:{number}: [{rule}] {line.strip()}")
    return findings


def check_phone_file(path, rel):
    findings = []
    for number, line in enumerate(code_lines(read(path)), start=1):
        m = IMPORT.match(line)
        if m and m.group(1) not in PHONE_IMPORTS:
            findings.append(f"{rel}:{number}: [phone-import] {line.strip()}")
    return findings


def run_checks(root):
    findings = []
    for sub in SHARED:
        base = os.path.join(root, sub)
        if not os.path.isdir(base):
            raise FileNotFoundError(f"no {sub} under {root}")
        for path in swift_files(base):
            findings += check_shared_file(path, os.path.relpath(path, root))
    base = os.path.join(root, PHONE_CORE)
    if os.path.isdir(base):
        for path in swift_files(base):
            findings += check_phone_file(path, os.path.relpath(path, root))
    return findings


KIT = "apps/macos/CapstanKit/Sources/CapstanKit/Planted.swift"
CORE = "apps/macos/CapstanAppCore/Sources/CapstanAppCore/Planted.swift"
PHONE = PHONE_CORE + "/CapstanPhoneCore/Planted.swift"

# (name, file, text, expect a finding)
CASES = [
    ("process", KIT, "func f() { let p = Process() }\n", True),
    ("kill", KIT, "func f() { kill(1, SIGTERM) }\n", True),
    ("workspace", CORE, "func f() { NSWorkspace.shared.open(url) }\n", True),
    ("sound", CORE, "func f() { NSSound.beep() }\n", True),
    ("appkit", CORE, "import AppKit\n", True),
    ("usr-bin", CORE, 'let tool = "/usr/bin/env"\n', True),
    ("ssh-path elsewhere", KIT, 'enum X { static let sshPath = "/usr/bin/ssh" }\n', True),
    ("else branch", KIT, "#if os(macOS)\nlet a = 1\n#else\nlet p = Process()\n#endif\n", True),
    ("nested if", KIT, "#if DEBUG\n#if true\nfunc f() { kill(0, 0) }\n#endif\n#endif\n", True),
    ("nested in else", CORE, "#if os(macOS)\n#else\n#if DEBUG\nfunc f() { NSSound.beep() }\n#endif\n#endif\n", True),
    ("elseif branch", KIT, "#if os(macOS)\n#elseif os(iOS)\nlet p = Process()\n#endif\n", True),
    ("not macOS", KIT, "#if !os(macOS)\nlet p = Process()\n#endif\n", True),
    ("after the block", KIT, "#if os(macOS)\nlet a = Process()\n#endif\nlet b = Process()\n", True),
    ("after a // string", KIT, 'let s = "a//b"; let p = Process()\n', True),
    ("phone UIKit", PHONE, "import UIKit\n", True),
    ("phone attr SwiftUI", PHONE, "@preconcurrency import SwiftUI\n", True),
    ("phone NIO", PHONE, "import NIOSSH\n", True),
    ("phone CapstanSSH", PHONE, "import struct CapstanSSH.SSHRunner\n", True),
    ("skill", KIT, "func f() { skill(1) }\n", False),
    ("live runner", KIT, "func f() { _ = LiveProcessRunner() }\n", False),
    ("runner", KIT, "func f(_ r: any ProcessRunner) { _ = ProcessRunner(r) }\n", False),
    ("inside macOS", KIT, "#if os(macOS)\nlet p = Process()\n#if DEBUG\nfunc f() { kill(0, 0) }\n#endif\n#endif\n", False),
    ("comments", CORE, "// Process() and /usr/bin/env\n/* NSWorkspace /* nested */ AppKit */\nlet a = 1\n", False),
    ("phone allowed", PHONE, "import Foundation\nimport CryptoKit\n@preconcurrency import Security\n"
     "import LocalAuthentication\nimport CapstanKit\nimport CapstanAppCore\n", False),
]


def self_test(root):
    try:
        clean = run_checks(root)
    except FileNotFoundError as e:
        print(f"self-test: {e}", file=sys.stderr)
        return 2
    if clean:
        print("self-test: the unmodified tree has findings:", file=sys.stderr)
        for line in clean:
            print(line, file=sys.stderr)
        return 1
    failed = 0
    with tempfile.TemporaryDirectory(prefix="check-shared-platform.") as tmp:
        for sub in SHARED:
            shutil.copytree(os.path.join(root, sub), os.path.join(tmp, sub))
        if os.path.isdir(os.path.join(root, PHONE_CORE)):
            shutil.copytree(os.path.join(root, PHONE_CORE), os.path.join(tmp, PHONE_CORE))
        for name, rel, text, expect in CASES:
            path = os.path.join(tmp, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w", encoding="utf-8") as f:
                f.write(text)
            found = [line for line in run_checks(tmp) if line.startswith(rel + ":")]
            os.remove(path)
            if bool(found) != expect:
                what = "was not caught" if expect else f"gave a finding: {found[0]}"
                print(f"self-test: '{name}' {what}", file=sys.stderr)
                failed += 1
    if failed:
        return 1
    print(f"self-test passed ({len(CASES)} planted files behaved as expected)")
    return 0


def main(argv):
    args = argv[1:]
    if args and args[0] == "--self-test":
        if len(args) > 2:
            return 2
        return self_test(os.path.abspath(args[1]) if len(args) == 2 else DEFAULT_ROOT)
    if len(args) > 1 or (args and args[0].startswith("-")):
        print("usage: check-shared-platform.py [repo root] | --self-test [repo root]", file=sys.stderr)
        return 2
    root = os.path.abspath(args[0]) if args else DEFAULT_ROOT
    try:
        findings = run_checks(root)
    except FileNotFoundError as e:
        print(str(e), file=sys.stderr)
        return 2
    for line in findings:
        print(line)
    if findings:
        return 1
    print("check-shared-platform: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

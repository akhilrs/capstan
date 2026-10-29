#!/usr/bin/env python3
"""Static guard for apps/macos/Capstan.xcodeproj. A guard, not a proof: Xcode is the real gate.

  check-xcodeproj.py [path/to/Capstan.xcodeproj]   check the project (default: next to this script's app)
  check-xcodeproj.py --self-test                   mutate copies of the project and expect each to fail

Exit 0 pass, 1 finding (first line of output names it), 2 usage.
"""
import os
import re
import shutil
import sys
import tempfile
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_PROJECT = os.path.join(HERE, "..", "Capstan.xcodeproj")
PACKAGES = ("CapstanKit", "CapstanAppCore")
ID = re.compile(r"\b[0-9A-F]{24}\b")


class Finding(Exception):
    pass


def need(cond, message):
    if not cond:
        raise Finding(message)


def read(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        raise Finding(f"cannot read {path}: {e.strerror}")


def strip_noise(text):
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return re.sub(r'"(?:[^"\\]|\\.)*"', '""', text)


def block(text, start):
    """The text of the {...} that opens at index start (a '{')."""
    depth = 0
    for i in range(start, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
    raise Finding("unterminated block")


def obj(text, oid):
    m = re.search(r"^\t\t" + oid + r"\b[^=\n]*= \{", text, flags=re.M)
    need(m, f"object {oid} is not defined")
    return block(text, m.end() - 1)


def id_list(body, key):
    m = re.search(key + r" = \((.*?)\);", body, flags=re.S)
    return ID.findall(m.group(1)) if m else []


def check_project(project):
    pbx = read(os.path.join(project, "project.pbxproj"))
    clean = strip_noise(pbx)
    need(clean.count("{") == clean.count("}"), "project.pbxproj has unbalanced braces")
    need(clean.count("(") == clean.count(")"), "project.pbxproj has unbalanced parentheses")

    need(re.search(r"^\tobjectVersion = 77;", pbx, flags=re.M), "objectVersion is not 77")

    defined = set(re.findall(r"^\t\t([0-9A-F]{24})\b[^=\n]*= \{", pbx, flags=re.M))
    for ref in sorted(set(ID.findall(pbx)) - defined):
        raise Finding(f"id {ref} is referenced but not defined")

    root = re.search(r"^\trootObject = ([0-9A-F]{24})", pbx, flags=re.M)
    need(root, "no rootObject")
    proj = obj(pbx, root.group(1))

    # one synchronized group, named Capstan
    groups = re.findall(r"^\t\t[0-9A-F]{24}\b[^=\n]*= \{\n\t\t\tisa = PBXFileSystemSynchronizedRootGroup;", pbx, flags=re.M)
    need(len(groups) == 1, f"expected one synchronized group, found {len(groups)}")
    need(re.search(r"PBXFileSystemSynchronizedRootGroup;\s*path = Capstan;", pbx), "the synchronized group is not Capstan")

    # package references: both local, each holding a Package.swift
    refs = id_list(proj, "packageReferences")
    paths = []
    for rid in refs:
        body = obj(pbx, rid)
        need("XCLocalSwiftPackageReference;" in body, f"package reference {rid} is not a local package")
        m = re.search(r"relativePath = ([^;]+);", body)
        need(m, f"package reference {rid} has no relativePath")
        paths.append(m.group(1).strip().strip('"'))
    need(sorted(paths) == sorted(PACKAGES), f"package references are {sorted(paths)}, expected {sorted(PACKAGES)}")
    base = os.path.dirname(os.path.abspath(project))
    for p in paths:
        need(os.path.isfile(os.path.join(base, p, "Package.swift")), f"{p} holds no Package.swift")

    # the Capstan app target and its build settings
    targets = [t for t in id_list(proj, "targets")]
    need(len(targets) == 1, "expected one target")
    target = obj(pbx, targets[0])
    need("com.apple.product-type.application" in target, "the target is not an application")
    need(re.search(r"name = Capstan;", target), "the target is not named Capstan")
    products = set(re.findall(r"productName = (\w+);", pbx))
    need(set(PACKAGES) <= products, "the package products are not both linked")
    lst = re.search(r"buildConfigurationList = ([0-9A-F]{24})", target)
    need(lst, "the target has no configuration list")
    configs = id_list(obj(pbx, lst.group(1)), "buildConfigurations")
    need(len(configs) >= 2, "the target has fewer than two configurations")
    for cid in configs:
        settings = obj(pbx, cid)
        for key, want in (
            ("MACOSX_DEPLOYMENT_TARGET", "26.0"),
            ("SWIFT_VERSION", "6.0"),
            ("GENERATE_INFOPLIST_FILE", "YES"),
            ("ENABLE_HARDENED_RUNTIME", "YES"),
        ):
            m = re.search(r"\b" + key + r" = ([^;]+);", settings)
            need(m and m.group(1).strip().strip('"') == want, f"{key} is not {want} in configuration {cid}")

    # no sandbox, no entitlements file anywhere
    need(not re.search(r"ENABLE_APP_SANDBOX = (YES|\"YES\")", pbx), "ENABLE_APP_SANDBOX is YES")
    need("CODE_SIGN_ENTITLEMENTS" not in pbx, "CODE_SIGN_ENTITLEMENTS is set")

    # shared scheme
    scheme_path = os.path.join(project, "xcshareddata", "xcschemes", "Capstan.xcscheme")
    try:
        tree = ET.parse(scheme_path)
    except (OSError, ET.ParseError) as e:
        raise Finding(f"shared scheme is not readable XML: {e}")
    sroot = tree.getroot()
    need(sroot.tag == "Scheme", "the shared scheme root is not Scheme")
    built = [r.get("BlueprintName") for r in sroot.findall("./BuildAction/BuildActionEntries/BuildActionEntry/BuildableReference")]
    need("Capstan" in built, "the shared scheme does not build Capstan")
    testables = sroot.find("./TestAction/Testables")
    need(testables is None or len(list(testables)) == 0, "the shared scheme has testables")


# --- self-test -------------------------------------------------------------------------------

def mutations():
    def sub(old, new):
        def f(text):
            need(old in text, f"self-test cannot find {old!r}")
            return text.replace(old, new, 1)
        return f

    def drop_last_brace(text):
        i = text.rstrip().rfind("}")
        return text[:i] + text[i + 1 :]

    return [
        ("dangling id", "project.pbxproj", sub("A10000000000000000000002 /* CapstanAppCore in Frameworks */,", "A100000000000000000000FF /* x */,")),
        ("sandbox on", "project.pbxproj", sub("ENABLE_APP_SANDBOX = NO;", "ENABLE_APP_SANDBOX = YES;")),
        ("wrong package path", "project.pbxproj", sub("relativePath = CapstanKit;", "relativePath = NoSuchKit;")),
        ("unbalanced brace", "project.pbxproj", drop_last_brace),
        ("objectVersion 76", "project.pbxproj", sub("objectVersion = 77;", "objectVersion = 76;")),
        ("scheme without Capstan", "xcshareddata/xcschemes/Capstan.xcscheme", lambda t: t.replace('BlueprintName = "Capstan"', 'BlueprintName = "Other"')),
    ]


def self_test(project):
    project = os.path.abspath(project)
    app = os.path.dirname(project)
    try:
        check_project(project)
    except Finding as e:
        print(f"self-test: the unmodified project fails: {e}", file=sys.stderr)
        return 1
    failed = 0
    cases = mutations()
    for name, rel, mutate in cases:
        with tempfile.TemporaryDirectory(prefix="check-xcodeproj.") as tmp:
            copy = os.path.join(tmp, "Capstan.xcodeproj")
            shutil.copytree(project, copy)
            for p in PACKAGES:
                os.symlink(os.path.join(app, p), os.path.join(tmp, p))
            target = os.path.join(copy, rel)
            try:
                with open(target, encoding="utf-8") as f:
                    text = f.read()
                with open(target, "w", encoding="utf-8") as f:
                    f.write(mutate(text))
            except Finding as e:
                print(f"self-test: {name}: {e}", file=sys.stderr)
                failed += 1
                continue
            try:
                check_project(copy)
            except Finding:
                continue
            print(f"self-test: '{name}' was not caught", file=sys.stderr)
            failed += 1
    if failed:
        return 1
    print(f"self-test passed ({len(cases)} mutated copies failed as expected)")
    return 0


def main(argv):
    args = argv[1:]
    if args and args[0] == "--self-test":
        if len(args) > 2:
            return 2
        return self_test(args[1] if len(args) == 2 else DEFAULT_PROJECT)
    if len(args) > 1 or (args and args[0].startswith("-")):
        print("usage: check-xcodeproj.py [Capstan.xcodeproj] | --self-test [Capstan.xcodeproj]", file=sys.stderr)
        return 2
    project = args[0] if args else DEFAULT_PROJECT
    try:
        check_project(project)
    except Finding as e:
        print(str(e), file=sys.stderr)
        return 1
    print("check-xcodeproj: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

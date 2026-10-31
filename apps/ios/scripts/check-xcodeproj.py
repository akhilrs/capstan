#!/usr/bin/env python3
"""Static guard for apps/ios/CapstanPhone.xcodeproj. A guard, not a proof: Xcode is the real gate.

  check-xcodeproj.py [path/to/CapstanPhone.xcodeproj]   check the project (default: next to this script's app)
  check-xcodeproj.py --self-test                        mutate copies of the project and expect each to fail

Exit 0 pass, 1 finding (first line of output names it), 2 usage.
"""
import os
import plistlib
import re
import shutil
import sys
import tempfile
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_PROJECT = os.path.join(HERE, "..", "CapstanPhone.xcodeproj")
PACKAGES = ("../macos/CapstanKit", "../macos/CapstanAppCore", "CapstanPhoneCore", "CapstanSSH")
PRODUCTS = ("CapstanKit", "CapstanAppCore", "CapstanPhoneCore", "CapstanSSH")
INFO_PLIST = "CapstanPhone/Info.plist"
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

    # one synchronized group, CapstanPhone, with Info.plist kept out of the target's files
    groups = re.findall(r"^\t\t([0-9A-F]{24})\b[^=\n]*= \{\n\t\t\tisa = PBXFileSystemSynchronizedRootGroup;", pbx, flags=re.M)
    need(len(groups) == 1, f"expected one synchronized group, found {len(groups)}")
    group = obj(pbx, groups[0])
    need(re.search(r"path = CapstanPhone;", group), "the synchronized group is not CapstanPhone")
    exceptions = id_list(group, "exceptions")
    need(exceptions, "the synchronized group keeps Info.plist as a resource (no exception set)")
    need(any(re.search(r"membershipExceptions = \(\s*Info\.plist,", obj(pbx, e)) for e in exceptions),
         "Info.plist is not excluded from the target's files")

    # package references: the four local packages, each holding a Package.swift
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

    # the CapstanPhone app target and its build settings
    targets = id_list(proj, "targets")
    need(len(targets) == 1, "expected one target")
    target = obj(pbx, targets[0])
    need("com.apple.product-type.application" in target, "the target is not an application")
    need(re.search(r"name = CapstanPhone;", target), "the target is not named CapstanPhone")
    linked = set(re.findall(r"productName = (\w+);", pbx))
    need(set(PRODUCTS) <= linked, f"the package products are not all linked: {sorted(set(PRODUCTS) - linked)}")
    frameworks = [p for p in id_list(target, "buildPhases") if "PBXFrameworksBuildPhase" in obj(pbx, p)]
    need(frameworks and len(id_list(obj(pbx, frameworks[0]), "files")) == len(PRODUCTS),
         "the Frameworks phase does not link the four products")
    lst = re.search(r"buildConfigurationList = ([0-9A-F]{24})", target)
    need(lst, "the target has no configuration list")
    configs = id_list(obj(pbx, lst.group(1)), "buildConfigurations")
    need(len(configs) >= 2, "the target has fewer than two configurations")
    for cid in configs:
        settings = obj(pbx, cid)
        for key, want in (
            ("IPHONEOS_DEPLOYMENT_TARGET", "26.0"),
            ("SDKROOT", "iphoneos"),
            ("SWIFT_VERSION", "6.0"),
            ("GENERATE_INFOPLIST_FILE", "YES"),
            ("INFOPLIST_FILE", INFO_PLIST),
            ("TARGETED_DEVICE_FAMILY", "1"),
        ):
            m = re.search(r"\b" + key + r" = ([^;]+);", settings)
            need(m and m.group(1).strip().strip('"') == want, f"{key} is not {want} in configuration {cid}")

    # no sandbox setting and no entitlements, so no push entitlement (D8)
    need("ENABLE_APP_SANDBOX" not in pbx, "ENABLE_APP_SANDBOX is set")
    need("CODE_SIGN_ENTITLEMENTS" not in pbx, "CODE_SIGN_ENTITLEMENTS is set")
    need("aps-environment" not in pbx, "the project names aps-environment")

    # Info.plist: the usage strings, background fetch only, the capstan URL scheme
    info_path = os.path.join(base, INFO_PLIST)
    try:
        with open(info_path, "rb") as f:
            info = plistlib.load(f)
    except (OSError, plistlib.InvalidFileException, ValueError) as e:
        raise Finding(f"{INFO_PLIST} is not a readable plist: {e}")
    for key in ("NSLocalNetworkUsageDescription", "NSFaceIDUsageDescription"):
        need(isinstance(info.get(key), str) and info[key].strip(), f"{INFO_PLIST} has no {key}")
    need(info.get("UIBackgroundModes") == ["fetch"], f"UIBackgroundModes is {info.get('UIBackgroundModes')}, expected ['fetch']")
    schemes = [s for t in info.get("CFBundleURLTypes", []) for s in t.get("CFBundleURLSchemes", [])]
    need(schemes == ["capstan"], f"the URL schemes are {schemes}, expected ['capstan']")
    need("aps-environment" not in info, f"{INFO_PLIST} names aps-environment")

    # shared scheme
    scheme_path = os.path.join(project, "xcshareddata", "xcschemes", "CapstanPhone.xcscheme")
    try:
        tree = ET.parse(scheme_path)
    except (OSError, ET.ParseError) as e:
        raise Finding(f"shared scheme is not readable XML: {e}")
    sroot = tree.getroot()
    need(sroot.tag == "Scheme", "the shared scheme root is not Scheme")
    built = [r.get("BlueprintName") for r in sroot.findall("./BuildAction/BuildActionEntries/BuildActionEntry/BuildableReference")]
    need("CapstanPhone" in built, "the shared scheme does not build CapstanPhone")
    ids = [r.get("BlueprintIdentifier") for r in sroot.iter("BuildableReference")]
    need(ids and all(i == targets[0] for i in ids), "the shared scheme points at another target id")


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

    pbx = "CapstanPhone.xcodeproj/project.pbxproj"
    return [
        ("dangling id", pbx, sub("B10000000000000000000002 /* CapstanAppCore in Frameworks */,", "B100000000000000000000FF /* x */,")),
        ("wrong package path", pbx, sub("relativePath = CapstanSSH;", "relativePath = NoSuchSSH;")),
        ("unbalanced brace", pbx, drop_last_brace),
        ("objectVersion 76", pbx, sub("objectVersion = 77;", "objectVersion = 76;")),
        ("deployment target 25", pbx, sub("IPHONEOS_DEPLOYMENT_TARGET = 26.0;\n\t\t\t\tLD_RUNPATH", "IPHONEOS_DEPLOYMENT_TARGET = 25.0;\n\t\t\t\tLD_RUNPATH")),
        ("macOS sdk", pbx, sub("SDKROOT = iphoneos;\n\t\t\t\tSWIFT_VERSION = 6.0;\n\t\t\t\tTARGETED", "SDKROOT = macosx;\n\t\t\t\tSWIFT_VERSION = 6.0;\n\t\t\t\tTARGETED")),
        ("sandbox set", pbx, sub("ENABLE_PREVIEWS = YES;", "ENABLE_PREVIEWS = YES;\n\t\t\t\tENABLE_APP_SANDBOX = NO;")),
        ("entitlements", pbx, sub("ENABLE_PREVIEWS = YES;", "ENABLE_PREVIEWS = YES;\n\t\t\t\tCODE_SIGN_ENTITLEMENTS = CapstanPhone/App.entitlements;")),
        ("Info.plist as a resource", pbx, sub("\t\t\texceptions = (\n\t\t\t\tB10000000000000000000033 /* Exceptions for \"CapstanPhone\" folder in \"CapstanPhone\" target */,\n\t\t\t);\n", "")),
        ("product not linked", pbx, sub("\t\t\t\tB10000000000000000000032 /* CapstanSSH in Frameworks */,\n", "")),
        ("scheme without CapstanPhone", "CapstanPhone.xcodeproj/xcshareddata/xcschemes/CapstanPhone.xcscheme",
         lambda t: t.replace('BlueprintName = "CapstanPhone"', 'BlueprintName = "Other"')),
        ("no Face ID string", INFO_PLIST, sub("<key>NSFaceIDUsageDescription</key>", "<key>NSFaceIDUsageDescriptionX</key>")),
        ("remote notification mode", INFO_PLIST, sub("<string>fetch</string>", "<string>fetch</string>\n\t\t<string>remote-notification</string>")),
        ("no URL scheme", INFO_PLIST, sub("<string>capstan</string>", "<string>other</string>")),
    ]


def self_test(project):
    project = os.path.abspath(project)
    ios = os.path.dirname(project)
    try:
        check_project(project)
    except Finding as e:
        print(f"self-test: the unmodified project fails: {e}", file=sys.stderr)
        return 1
    failed = 0
    cases = mutations()
    for name, rel, mutate in cases:
        with tempfile.TemporaryDirectory(prefix="check-xcodeproj.") as tmp:
            copy_ios = os.path.join(tmp, "ios")
            os.makedirs(os.path.join(copy_ios, "CapstanPhone"))
            shutil.copytree(project, os.path.join(copy_ios, "CapstanPhone.xcodeproj"))
            shutil.copy(os.path.join(ios, INFO_PLIST), os.path.join(copy_ios, INFO_PLIST))
            for p in ("CapstanPhoneCore", "CapstanSSH"):
                os.symlink(os.path.join(ios, p), os.path.join(copy_ios, p))
            os.makedirs(os.path.join(tmp, "macos"))
            for p in ("CapstanKit", "CapstanAppCore"):
                os.symlink(os.path.join(ios, "..", "macos", p), os.path.join(tmp, "macos", p))
            target = os.path.join(copy_ios, rel)
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
                check_project(os.path.join(copy_ios, "CapstanPhone.xcodeproj"))
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
        print("usage: check-xcodeproj.py [CapstanPhone.xcodeproj] | --self-test [CapstanPhone.xcodeproj]", file=sys.stderr)
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

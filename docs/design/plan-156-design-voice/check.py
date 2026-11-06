#!/usr/bin/env python3
"""Checks strings.json for plan-156 design-voice (standard library only).

The key list below is the design's contract: strings.json must hold exactly these keys, and the
app packages use only these. Run before reporting; the reviewer reruns it, and voice-intents wires
it into check-static.sh.

Fails when:
1. strings.json is not valid JSON or not a flat object of strings;
2. a key is missing from strings.json or not in REQUIRED (an extra key);
3. any string holds a full-auto word (FULL_AUTO), so the static rule can stay strict;
4. an announcement (notify.*) names a type of agent or holds a placeholder other than {project};
5. a fixed spoken line is over LINE_CAP characters (placeholders filled with long examples; the
   untrusted item text {command}, {prefix} is filled with nothing, since ITEM_CAP counts it
   on its own);
6. the frame or a refusal key the package names is missing, or the frame has the wrong words;
7. the note does not state LINE_CAP and ITEM_CAP as the same literal numbers;
8. once apps/ios/CapstanPhone/Voice/Shortcuts.swift exists, a phrase.* value is not a string literal
   there ({app} is written as the Swift interpolation of .applicationName). Only phrase.* values are compared.
Run: python3 docs/design/plan-156-design-voice/check.py
"""
import json
import pathlib
import re
import sys

LINE_CAP = 120  # fixed words of one spoken line, in characters
ITEM_CAP = 120  # the item text alone (command, prefix, project name), in characters; 80..160

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[2]
NOTE = HERE.parent / "plan-156-design-voice.md"
SHORTCUTS = ROOT / "apps/ios/CapstanPhone/Voice/Shortcuts.swift"

FULL_AUTO = re.compile(r"[Ff]ull[ _-]?[Aa]uto|autoAll|approveAll")
AGENT_WORDS = re.compile(r"\b(operator|pm|agent|worker|developer|architect|reviewer|designer|supervisor)s?\b", re.I)
FRAME_KEY = "frame.command"
FRAME_TEXT = "The command is: {command}. End of command."
NAMED = [
    FRAME_KEY,
    "refusal.locked",        # unreadable before the first unlock
    "refusal.notPaired",     # not paired
    "refusal.changed",       # changed or gone
    "refusal.unreachable",   # host unreachable, with its name
    "refusal.voiceOff",      # voice decisions off
    "refusal.notDecidable",  # not decidable by voice, and why
    "refusal.tooLong",       # too long to read
    "refusal.busy",          # busy
    "refusal.nothing",       # nothing to decide
]
EXAMPLES = {
    "project": "customer-billing-reconciliation",
    "host": "build-host-eu-west-2",
    "where": "customer-billing-reconciliation on build-host-eu-west-2",
    "n": "12",
    "kind": "A permission prompt",
    "age": "12 minutes ago",
    "reason": "it has characters that can't be read aloud",
    "app": "Capstan",
    "number": "5",
    "label": "commands that start the same way, for the session",
    "time": "16:58",
    "windowId": "win-17",
    "command": "",  # untrusted item text: ITEM_CAP, counted on its own
    "prefix": "",
}
REQUIRED = [
    'notify.op.title',
    'notify.op.body',
    'notify.prompt.title',
    'notify.prompt.body',
    'notify.design.title',
    'notify.design.body',
    'notify.message.title',
    'notify.message.body',
    'notify.integration.title',
    'notify.integration.body',
    'notify.other.title',
    'notify.other.body',
    'shortcut.list.title',
    'shortcut.approveOnce.title',
    'shortcut.approve.title',
    'shortcut.deny.title',
    'phrase.list',
    'phrase.approveOnce',
    'phrase.approve',
    'phrase.deny',
    'list.count.none',
    'list.count.one',
    'list.count.many',
    'list.item.plain',
    'list.item.long',
    'list.item.unverified',
    'list.more.one',
    'list.more.many',
    'list.hint',
    'list.unreachable',
    'list.unreachable.all',
    'list.item.unreadable',
    'kind.op',
    'kind.prompt',
    'kind.design',
    'kind.message',
    'kind.integration',
    'kind.other',
    'age.now',
    'age.minute',
    'age.minutes',
    'age.hour',
    'age.hours',
    'age.day',
    'age.days',
    'where.project',
    'where.projectHost',
    'where.unreadable',
    'frame.command',
    'prompt.choice',
    'prompt.choice.option',
    'prompt.prefix',
    'prompt.prefix.option',
    'prompt.secondYes.hour',
    'prompt.secondYes.session',
    'choose.label.once',
    'choose.label.exact',
    'choose.label.prefix',
    'choose.label.hour',
    'choose.label.session',
    'choose.onlyOnce',
    'choose.grantsOff',
    'choose.oldHost',
    'choose.unclear',
    'readback.approve',
    'readback.approveAnd',
    'readback.deny',
    'readback.skipped.one',
    'readback.skipped.many',
    'readback.exact',
    'readback.prefix',
    'readback.prefix.rule',
    'readback.prefix.length',
    'readback.hour',
    'readback.session',
    'readback.coverage.all',
    'readback.coverage.floor',
    'readback.coverage.scripts',
    'readback.coverage.tools',
    'confirm.approve',
    'confirm.approveAnd',
    'confirm.window',
    'confirm.deny',
    'confirm.cancelled',
    'result.approved',
    'result.exact',
    'result.exact.noEnd',
    'result.prefix',
    'result.prefix.noEnd',
    'result.window',
    'result.window.noEnd',
    'result.window.revoke',
    'result.forwarded',
    'result.denied',
    'result.unknown',
    'refusal.changed',
    'refusal.unreachable',
    'refusal.timeout',
    'refusal.voiceOff',
    'refusal.denyOnly',
    'refusal.notDecidable',
    'refusal.tooLong',
    'refusal.nothing',
    'refusal.readOnly.one',
    'refusal.readOnly.many',
    'refusal.busy',
    'refusal.needsTerminal',
    'refusal.limitReached',
    'refusal.noActivePm',
    'refusal.other',
    'refusal.locked',
    'refusal.notPaired',
    'reason.floor',
    'reason.restart',
    'reason.truncated',
    'reason.text_mismatch',
    'reason.not_checked',
    'reason.shell',
    'reason.multiline',
    'reason.unverified',
    'reason.nonAscii',
    'reason.project',
    'settings.voice.header',
    'settings.voice.toggle',
    'settings.grants.toggle',
    'settings.voice.footer',
    'settings.grants.footer',
    'settings.voice.phrases',
    'settings.timeSensitive.header',
    'settings.timeSensitive.footer',
]

fail = []
strings = json.loads((HERE / "strings.json").read_text(encoding="utf-8"))
if not isinstance(strings, dict) or not all(isinstance(v, str) for v in strings.values()):
    sys.exit("FAIL strings.json is not a flat object of strings")
if not 80 <= ITEM_CAP <= 160:
    fail.append(f"ITEM_CAP {ITEM_CAP} is outside 80..160")
if LINE_CAP > 120:
    fail.append(f"LINE_CAP {LINE_CAP} is over 120")
for key in REQUIRED:
    if key not in strings:
        fail.append(f"missing key {key}")
for key in strings:
    if key not in REQUIRED:
        fail.append(f"extra key, not in the contract: {key}")
for key in NAMED:
    if key not in strings or key not in REQUIRED:
        fail.append(f"named key missing: {key}")
if strings.get(FRAME_KEY) != FRAME_TEXT:
    fail.append(f"{FRAME_KEY} must be exactly: {FRAME_TEXT}")

longest = (0, "")
for key, text in strings.items():
    if FULL_AUTO.search(key) or FULL_AUTO.search(text):
        fail.append(f"{key}: holds a full-auto word")
    if "\n" in text:
        fail.append(f"{key}: has a line break")
    holes = re.findall(r"\{(\w+)\}", text)
    for h in holes:
        if h not in EXAMPLES:
            fail.append(f"{key}: unknown placeholder {{{h}}}")
    if key.startswith("settings."):
        continue  # shown on screen, not spoken
    filled = text
    for h in holes:
        filled = filled.replace("{" + h + "}", EXAMPLES.get(h, ""))
    if key.startswith("notify."):
        if AGENT_WORDS.search(text):
            fail.append(f"{key}: names a type of agent")
        if set(holes) - {"project"}:
            fail.append(f"{key}: an announcement may only hold {{project}}")
        filled = "Capstan: " + filled  # Siri says the app name first
    if len(filled) > LINE_CAP:
        fail.append(f"{key}: {len(filled)} characters > {LINE_CAP}: {filled}")
    if len(filled) > longest[0]:
        longest = (len(filled), key)

note = NOTE.read_text(encoding="utf-8") if NOTE.exists() else ""
for name, value in (("LINE_CAP", LINE_CAP), ("ITEM_CAP", ITEM_CAP)):
    if not re.search(rf"\b{name} = {value}\b", note):
        fail.append(f"the note does not state {name} = {value}")

phrases = {k: v for k, v in strings.items() if k.startswith("phrase.")}
if SHORTCUTS.exists():
    swift = SHORTCUTS.read_text(encoding="utf-8")
    literals = set(re.findall(r'"((?:[^"\\]|\\.)*)"', swift))
    for key, text in phrases.items():
        want = text.replace("{app}", "\\(.applicationName)")
        if want not in literals:
            fail.append(f"{key}: not a string literal in Shortcuts.swift: {want}")
    shortcuts = "checked"
else:
    shortcuts = "not present yet, phrase check skipped"

print(f"{len(strings)} keys, {len(REQUIRED)} in the contract; longest fixed line {longest[0]} characters "
      f"({longest[1]}); LINE_CAP {LINE_CAP}, ITEM_CAP {ITEM_CAP}; Shortcuts.swift {shortcuts}")
for f in fail:
    print("FAIL", f)
sys.exit(1 if fail else 0)

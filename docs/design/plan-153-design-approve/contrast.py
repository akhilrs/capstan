#!/usr/bin/env python3
"""Contrast of every colour pair the plan-153 approve boards use (standard library only).

Values are the approved app tokens: docs/design/iphone-app.md (iPhone) and docs/design/mac-app.md (Mac).
A colour with alpha is composited over the layers below it, top first in each row's stack.
Writes contrast.csv next to this file and exits 1 when any checked row is below its threshold.
Disabled controls are listed with threshold "exempt" (WCAG 1.4.3 and 1.4.11 exclude inactive components).
"""
import csv
import os
import sys

IOS = {
    "light": {"ground": "#efefec", "cell": "#ffffff", "label": "#1c2024", "label2": "#545b63", "sep": "rgba(28,32,36,.12)",
              "fill": "rgba(28,32,36,.04)", "fill2": "rgba(28,32,36,.08)", "accent": "#8a521c", "accent-tint": "rgba(138,82,28,.13)",
              "on-accent": "#ffffff", "red": "#a3221a", "red-tint": "rgba(179,38,30,.09)", "green": "#1d6b35",
              "green-tint": "rgba(29,107,53,.10)", "yellow": "#6b5100", "yellow-tint": "rgba(122,92,0,.10)",
              "focus": "#1d5bb8", "mono-bg": "rgba(28,32,36,.045)"},
    "dark": {"ground": "#101214", "cell": "#1b1d20", "label": "#eceef0", "label2": "#a6adb4", "sep": "rgba(255,255,255,.10)",
             "fill": "rgba(255,255,255,.05)", "fill2": "rgba(255,255,255,.10)", "accent": "#e0a45e", "accent-tint": "rgba(224,164,94,.16)",
             "on-accent": "#1a1206", "red": "#ffa49c", "red-tint": "rgba(255,123,112,.12)", "green": "#6fd68b",
             "green-tint": "rgba(111,214,139,.12)", "yellow": "#f0c95a", "yellow-tint": "rgba(240,201,90,.12)",
             "focus": "#7fb0ff", "mono-bg": "rgba(255,255,255,.05)"},
}
MAC = {
    "light": {"win": "#f6f6f4", "label": "#1c2024", "label2": "#545b63", "fill": "rgba(28,32,36,.04)", "fill2": "rgba(28,32,36,.08)",
              "accent": "#8a521c", "accent-tint": "rgba(138,82,28,.13)", "on-accent": "#ffffff", "red": "#a3221a",
              "red-tint": "rgba(179,38,30,.09)", "green-tint": "rgba(29,107,53,.10)", "yellow-tint": "rgba(122,92,0,.10)",
              "focus": "#1d5bb8", "mono-bg": "rgba(28,32,36,.045)"},
    "dark": {"win": "#1b1d20", "label": "#eceef0", "label2": "#a6adb4", "fill": "rgba(255,255,255,.04)", "fill2": "rgba(255,255,255,.09)",
             "accent": "#e0a45e", "accent-tint": "rgba(224,164,94,.16)", "on-accent": "#1a1206", "red": "#ffa49c",
             "red-tint": "rgba(255,123,112,.12)", "green-tint": "rgba(111,214,139,.12)", "yellow-tint": "rgba(240,201,90,.12)",
             "focus": "#7fb0ff", "mono-bg": "rgba(255,255,255,.05)"},
}

# (id, app, what, foreground token, background stack top→bottom, kind)
# kind: text (4.5), ui (3.0: control boundaries, focus rings, selection marks), disabled (exempt)
PAIRS = [
    ("T01", "ios", "Sheet title, card titles, command text", "label", ["cell"], "text"),
    ("T02", "ios", "Card details, notes, step count", "label2", ["cell"], "text"),
    ("T03", "ios", "Detail notes and meta labels on the ground", "label2", ["ground"], "text"),
    ("T04", "ios", "Back and Cancel (plain accent buttons)", "accent", ["cell"], "text"),
    ("T05", "ios", "Primary button text (Answer…, Continue, Approve…)", "on-accent", ["accent"], "text"),
    ("T06", "ios", "Selected card title", "label", ["accent-tint", "cell"], "text"),
    ("T07", "ios", "Selected card detail", "label2", ["accent-tint", "cell"], "text"),
    ("T08", "ios", "Prefix value in its box (selected card)", "label", ["mono-bg", "accent-tint", "cell"], "text"),
    ("T09", "ios", "One value, shown whole (selected card)", "label2", ["accent-tint", "cell"], "text"),
    ("T10", "ios", "Deny… button text", "red", ["fill2", "ground"], "text"),
    ("T11", "ios", "Limit line and coverage text", "label", ["fill2", "cell"], "text"),
    ("T12", "ios", "Floor entry label, end text in coverage", "label2", ["fill2", "cell"], "text"),
    ("T13", "ios", "Result lead on green tint", "label", ["green-tint", "ground"], "text"),
    ("T14", "ios", "Result small print on green tint", "label2", ["green-tint", "ground"], "text"),
    ("T15", "ios", "Refusal lead on red tint", "label", ["red-tint", "cell"], "text"),
    ("T16", "ios", "Refusal small print on red tint", "label2", ["red-tint", "cell"], "text"),
    ("T17", "ios", "busy lead on yellow tint", "label", ["yellow-tint", "cell"], "text"),
    ("T18", "ios", "busy small print on yellow tint", "label2", ["yellow-tint", "cell"], "text"),
    ("T19", "ios", "Host message in its block on red tint", "label", ["mono-bg", "red-tint", "cell"], "text"),
    ("T20", "ios", "Denied result on grey", "label2", ["fill2", "ground"], "text"),
    ("U01", "ios", "Radio ring, not chosen", "label2", ["cell"], "ui"),
    ("U02", "ios", "Selection border of the chosen card", "accent", ["cell"], "ui"),
    ("U03", "ios", "Radio dot of the chosen card", "accent", ["accent-tint", "cell"], "ui"),
    ("U04", "ios", "Focus ring on the sheet", "focus", ["cell"], "ui"),
    ("U05", "ios", "Focus ring on the detail ground", "focus", ["ground"], "ui"),
    ("U06", "ios", "Primary button edge (accent fill)", "accent", ["cell"], "ui"),
    ("D01", "ios", "Disabled primary (Choose an answer, after a refusal)", "label2", ["fill2", "cell"], "disabled"),
    ("T21", "mac", "Sheet title, radio titles, command text", "label", ["win"], "text"),
    ("T22", "mac", "Radio details, notes, step count", "label2", ["win"], "text"),
    ("T23", "mac", "Radio row title on its fill", "label", ["fill", "win"], "text"),
    ("T24", "mac", "Radio row detail on its fill", "label2", ["fill", "win"], "text"),
    ("T25", "mac", "Primary button text", "on-accent", ["accent"], "text"),
    ("T26", "mac", "Secondary button text (Back, Cancel)", "label", ["fill2", "win"], "text"),
    ("T27", "mac", "Deny… button text", "red", ["fill2", "win"], "text"),
    ("T28", "mac", "Selected radio detail", "label2", ["accent-tint", "win"], "text"),
    ("T29", "mac", "Prefix value in its box (selected row)", "label", ["mono-bg", "accent-tint", "win"], "text"),
    ("T30", "mac", "Result small print on green tint", "label2", ["green-tint", "win"], "text"),
    ("T31", "mac", "Refusal small print on red tint", "label2", ["red-tint", "win"], "text"),
    ("T32", "mac", "busy small print on yellow tint", "label2", ["yellow-tint", "win"], "text"),
    ("T33", "mac", "Limit line small print on grey", "label2", ["fill2", "win"], "text"),
    ("U07", "mac", "Radio ring, not chosen", "label2", ["fill", "win"], "ui"),
    ("U08", "mac", "Selection border of the chosen row", "accent", ["win"], "ui"),
    ("U09", "mac", "Radio dot of the chosen row", "accent", ["accent-tint", "win"], "ui"),
    ("U10", "mac", "Focus ring", "focus", ["win"], "ui"),
    ("D02", "mac", "Disabled primary (after a refusal, while sending)", "label2", ["fill2", "win"], "disabled"),
]

THRESHOLD = {"text": 4.5, "ui": 3.0}


def rgba(v):
    v = v.strip()
    if v.startswith("#"):
        return (int(v[1:3], 16), int(v[3:5], 16), int(v[5:7], 16), 1.0)
    parts = [float(x) for x in v[v.index("(") + 1:v.index(")")].split(",")]
    return (parts[0], parts[1], parts[2], parts[3] if len(parts) > 3 else 1.0)


def over(top, below):
    a = top[3]
    return tuple(top[i] * a + below[i] * (1 - a) for i in range(3)) + (1.0,)


def composite(tokens, stack):
    colour = rgba(tokens[stack[-1]])
    for name in reversed(stack[:-1]):
        colour = over(rgba(tokens[name]), colour)
    return colour


def luminance(c):
    def ch(v):
        v /= 255
        return v / 12.92 if v <= 0.03928 else ((v + 0.055) / 1.055) ** 2.4
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])


def ratio(a, b):
    la, lb = luminance(a), luminance(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def main():
    rows, failed = [], 0
    for pid, app, what, fg, stack, kind in PAIRS:
        for theme in ("light", "dark"):
            tokens = (IOS if app == "ios" else MAC)[theme]
            bg = composite(tokens, stack)
            r = ratio(over(rgba(tokens[fg]), bg), bg)
            th = THRESHOLD.get(kind)
            ok = "exempt" if th is None else ("pass" if r >= th else "FAIL")
            failed += ok == "FAIL"
            rows.append([pid, app, theme, what, fg, " over ".join(stack), kind, "exempt" if th is None else f"{th:.1f}", f"{r:.2f}", ok])
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "contrast.csv")
    with open(path, "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["id", "app", "theme", "use", "foreground", "background", "kind", "threshold", "ratio", "result"])
        w.writerows(rows)
    for r in rows:
        print(f"{r[0]:4} {r[1]:3} {r[2]:5} {r[8]:>6} {r[7]:>6} {r[9]:6} {r[3]}")
    print(f"{len(rows)} rows, {failed} below threshold")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())

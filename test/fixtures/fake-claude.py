#!/usr/bin/env python3
"""A stand-in for Claude Code that draws the real screens the Herdr adapter reads.

It shows the real trust dialog for its working directory, then the real idle
screen with an editable input line. Received prompts are appended to the file
named by FAKE_CLAUDE_LOG and its arguments are written to FAKE_CLAUDE_ARGS.
It makes no network calls and never runs a model.
"""
import os
import re
import sys
import termios
import tty

HERE = os.environ.get("FAKE_CLAUDE_FIXTURES") or os.path.dirname(os.path.realpath(__file__))
DIALOG = open(os.path.join(HERE, "claude-trust-dialog.txt"), encoding="utf-8").read().split("\n")
LOG = os.environ.get("FAKE_CLAUDE_LOG")
ARGS_LOG = os.environ.get("FAKE_CLAUDE_ARGS")
NBSP = " "
RULE = "─" * 60
OPTIONS = ["No, exit", "Yes, I trust this folder"]


def out(text):
    sys.stdout.write(text.replace("\r\n", "\n").replace("\n", "\r\n"))
    sys.stdout.flush()


def draw_dialog(selected):
    lines = []
    for line in DIALOG:
        stripped = line.strip().lstrip("❯").strip()
        if stripped in OPTIONS:
            marker = "❯ " if OPTIONS.index(stripped) == selected else "  "
            lines.append(f" {marker}{stripped}" if marker.strip() else f"   {stripped}")
        else:
            lines.append(line)
    for index, line in enumerate(lines):
        if line.strip() == "Accessing workspace:":
            lines[index + 2] = f" {os.getcwd()}"
    out("\x1b[2J\x1b[H" + "\n".join(lines))


def draw_idle(lines, received):
    if any(lines):
        line = f"❯{NBSP}{lines[0]}" + "".join(f"\n  {extra}" for extra in lines[1:])
    else:
        line = f"❯{NBSP}\x1b[0m\x1b[2mTry \"how do I log an error?\"\x1b[0m"
    out(
        "\x1b[2J\x1b[H"
        " ▐▛███▛█   Claude Code v2.1.285\n"
        "▝▜██████▛   Fake model\n\n"
        + "".join(f"RECEIVED: {text}\n" for text in received)
        + f"{RULE}\n{line}\n{RULE}\n"
        "  ⏵⏵ manual mode on (shift+tab to cycle)\n"
    )


def read_key(fd):
    return os.read(fd, 4096).decode("utf-8", errors="ignore")


def main():
    if ARGS_LOG:
        with open(ARGS_LOG, "w", encoding="utf-8") as handle:
            handle.write("\n".join(sys.argv[1:]) + "\n")
    fd = sys.stdin.fileno()
    old = termios.tcgetattr(fd)
    tty.setraw(fd)
    try:
        selected = 0
        draw_dialog(selected)
        while True:
            key = read_key(fd)
            if key.startswith("\x1b[B") or key.startswith("\x1b[A"):
                selected = 1 - selected
                draw_dialog(selected)
            elif key in ("\r", "\n"):
                if selected == 0:
                    return
                break
        lines, received = [""], []
        draw_idle(lines, received)
        while True:
            chunk = read_key(fd)
            chunk = chunk.replace("\x1b[200~", "").replace("\x1b[201~", "")
            for char in re.findall(r"\x1b\[[0-9;]*[A-Za-z]|.", chunk, flags=re.S):
                if char == "\x15":
                    if lines[-1]:
                        lines[-1] = ""
                    elif len(lines) > 1:
                        lines.pop()
                elif char in ("\x7f", "\x08"):
                    lines[-1] = lines[-1][:-1]
                elif char == "\r":
                    text = "\n".join(lines)
                    if text.strip():
                        received.append(text.replace("\n", " / "))
                        if LOG:
                            with open(LOG, "a", encoding="utf-8") as handle:
                                handle.write(text + "\n---\n")
                    lines = [""]
                elif char == "\x03":
                    return
                elif char == "\n":
                    lines.append("")
                elif len(char) == 1 and char >= " ":
                    lines[-1] += char
            draw_idle(lines, received)
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old)


main()

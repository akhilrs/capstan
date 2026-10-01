import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  TRUST_NO,
  TRUST_YES,
  CODEX_TRUST_NO,
  CODEX_TRUST_YES,
  extractInputLine,
  freshPromptReady,
  parseCodexTrustDialog,
  parseTrustDialog,
  parseTrustDialogOf,
  trustTexts,
  stripAnsi,
} from "../src/herdr/screen.js";

const fixture = (name: string): string =>
  readFileSync(path.resolve("test/fixtures", name), "utf8");

const RULE = "─".repeat(40);
const NBSP = " ";
const RESET = "\u001b[0m";
const DIM = "\u001b[2m";

function claudeScreen(...regionLines: string[]): string {
  return ["header art", "", RULE, ...regionLines, RULE, "  footer"].join(
    "\r\n",
  );
}

test("the real empty idle screens read as empty and the real typed screen as its text", () => {
  for (const name of ["claude-idle-empty.ansi", "claude-idle-clean-env.ansi"])
    assert.equal(extractInputLine("claude", fixture(name)), "", name);
  assert.equal(
    extractInputLine("claude", fixture("claude-idle-typed.ansi")),
    "hello typed",
  );
});

test("the plain read of an empty idle screen cannot be told from typed text, which is why the ANSI read is required", () => {
  const plain = fixture("claude-idle-empty.txt");
  assert.match(plain, /❯[ \u00a0]Try "/);
  assert.equal(
    extractInputLine("claude", fixture("claude-idle-empty.ansi")),
    "",
  );
  assert.equal(
    extractInputLine("claude", plain),
    'Try "how do I log an error?"',
  );
});

test("a dim placeholder is empty whatever attribute codes surround it", () => {
  for (const line of [
    `❯${NBSP}${RESET}${DIM}Try "x"${RESET}`,
    `❯${NBSP}${DIM}Try "x"${RESET}`,
    `\u001b[38;2;1;2;3m❯${RESET}${NBSP}${RESET}${DIM}Try "x" ${RESET}${NBSP}${RESET}`,
    `❯${NBSP}`,
    "❯",
  ])
    assert.equal(
      extractInputLine("claude", claudeScreen(line)),
      "",
      JSON.stringify(line),
    );
});

test("text that is not entirely dim is typed input, including colored text whose color code contains a 2", () => {
  for (const [line, text] of [
    [`❯${NBSP}hello`, "hello"],
    [`❯${NBSP}${DIM}Try${RESET} more`, "Try more"],
    [`❯${NBSP}\u001b[38;2;215;119;87mcolored${RESET}`, "colored"],
    [`❯${NBSP}\u001b[48;5;2mbg${RESET}`, "bg"],
    [`❯${NBSP}x${DIM}Try${RESET}`, "xTry"],
  ] as const)
    assert.equal(
      extractInputLine("claude", claudeScreen(line)),
      text,
      JSON.stringify(line),
    );
});

test("multi-line input, a blank first line and a pasted-text indicator all count as input", () => {
  assert.equal(
    extractInputLine(
      "claude",
      claudeScreen(`❯${NBSP}first`, "  second", "  third"),
    ),
    "first\n  second\n  third",
  );
  assert.equal(
    extractInputLine(
      "claude",
      claudeScreen(`❯${NBSP}`, "  hidden second line"),
    ),
    "\n  hidden second line",
  );
  assert.equal(
    extractInputLine(
      "claude",
      claudeScreen(`❯${NBSP}[Pasted text #1 +12 lines]`),
    ),
    "[Pasted text #1 +12 lines]",
  );
  assert.equal(
    extractInputLine(
      "claude",
      claudeScreen(`❯${NBSP}${DIM}Try "x"${RESET}`, "   "),
    ),
    "",
  );
});

test("a screen without a readable input box fails closed", () => {
  assert.equal(
    extractInputLine("claude", fixture("claude-trust-dialog.txt")),
    undefined,
  );
  assert.equal(extractInputLine("claude", ""), undefined);
  assert.equal(
    extractInputLine("claude", ["a", RULE, "❯ x", "b"].join("\n")),
    undefined,
    "one rule line",
  );
  assert.equal(
    extractInputLine("claude", claudeScreen("no marker here")),
    undefined,
  );
  assert.equal(
    extractInputLine("claude", [RULE, RULE].join("\n")),
    undefined,
    "adjacent rules",
  );
  assert.equal(
    extractInputLine("codex", fixture("claude-idle-typed.ansi")),
    undefined,
  );
  assert.equal(extractInputLine("omp", claudeScreen(`❯${NBSP}x`)), undefined);
});

test("a typed line made of rule characters moves the region and fails closed instead of reading empty", () => {
  const screen = [
    "",
    RULE,
    `❯${NBSP}first line`,
    "─".repeat(20),
    "  after the fake rule",
    RULE,
    "  footer",
  ].join("\r\n");
  assert.equal(extractInputLine("claude", screen), undefined);
  assert.notEqual(extractInputLine("claude", screen), "");
});

test("a prompt marker outside the input box is ignored", () => {
  const screen = [
    "❯ claude",
    "❯ No, exit",
    "",
    RULE,
    `❯${NBSP}${DIM}Try${RESET}`,
    RULE,
    "  ❯ menu",
  ].join("\r\n");
  assert.equal(extractInputLine("claude", screen), "");
});

test("the shell reader needs the prompt on the last line and accepts a bare marker", () => {
  assert.equal(extractInputLine("shell", "user in dir\n❯ \n"), "");
  assert.equal(extractInputLine("shell", "user in dir\n❯\n"), "");
  assert.equal(
    extractInputLine("shell", "user in dir\n❯ half typed\n"),
    "half typed",
  );
  assert.equal(extractInputLine("shell", `x\n${"\u001b[1m"}❯${RESET} \n`), "");
  assert.equal(extractInputLine("shell", "❯ ls\noutput line\n"), undefined);
  assert.equal(extractInputLine("shell", "\n\n"), undefined);
  assert.equal(extractInputLine("shell", "user@host:~$ "), undefined);
});

test("a fresh pane is ready only when the last line is exactly one prompt symbol", () => {
  for (const ready of ["a\n❯", "a\n❯ \n\n", "b\n$", "b\n #  ", "c\n%"])
    assert.equal(freshPromptReady(ready), true, JSON.stringify(ready));
  for (const notReady of [
    "❯ half typed",
    "echo $",
    "user@host:~$",
    "user@host:~$ ",
    "",
    "\n\n",
    "❯\nrunning",
    "❯❯",
    "$ ls",
  ])
    assert.equal(freshPromptReady(notReady), false, JSON.stringify(notReady));
  assert.equal(freshPromptReady(fixture("claude-idle-empty.txt")), false);
});

test("the real trust dialog parses into its path and its two options", () => {
  const dialog = parseTrustDialog(fixture("claude-trust-dialog.txt"));
  assert.ok(dialog && dialog.kind === "dialog");
  assert.equal(
    dialog.path,
    "/home/user/.herdr/worktrees/probe-repo-FQ8H/probe-two",
  );
  assert.deepEqual(dialog.options, [
    { text: TRUST_NO, selected: true },
    { text: TRUST_YES, selected: false },
  ]);
  assert.equal(dialog.selectedIndex, 0);
  assert.equal(dialog.confirmIsLastLine, true);
});

test("the dialog parser follows the selection marker and rejects odd dialogs", () => {
  const text = fixture("claude-trust-dialog.txt");
  const moved = text
    .replace(`❯ ${TRUST_NO}`, `  ${TRUST_NO}`)
    .replace(`  ${TRUST_YES}`, `❯ ${TRUST_YES}`);
  const dialog = parseTrustDialog(moved);
  assert.ok(dialog && dialog.kind === "dialog");
  assert.equal(dialog.selectedIndex, 1);
  const both = parseTrustDialog(
    text.replace(`  ${TRUST_YES}`, `❯ ${TRUST_YES}`),
  );
  assert.ok(both && both.kind === "dialog");
  assert.equal(both.selectedIndex, undefined);
  const none = parseTrustDialog(text.replace(`❯ ${TRUST_NO}`, `  ${TRUST_NO}`));
  assert.ok(none && none.kind === "dialog");
  assert.equal(none.selectedIndex, undefined);
  assert.deepEqual(
    parseTrustDialog(
      text.replace(
        "probe-repo-FQ8H/probe-two",
        "probe-repo-FQ8H/probe-two\n   -continued",
      ),
    ),
    { kind: "wrapped_path" },
  );
  assert.equal(
    parseTrustDialog(text.replace("Enter to confirm", "Press a key")),
    undefined,
  );
  assert.equal(parseTrustDialog("nothing to see"), undefined);
  assert.equal(
    parseTrustDialog(
      text.replace("Accessing workspace:", "Accessing something:"),
    ),
    undefined,
  );
});

test("only the block above the confirm line is options, and a dialog that is not the last thing on screen is flagged", () => {
  const text = fixture("claude-trust-dialog.txt");
  const dialog = parseTrustDialog(text);
  assert.ok(dialog && dialog.kind === "dialog");
  assert.ok(
    !dialog.options.some((option) => /Security guide/.test(option.text)),
  );
  const later = parseTrustDialog(`${text}\nsomething printed afterwards\n`);
  assert.ok(later && later.kind === "dialog");
  assert.equal(later.confirmIsLastLine, false);
  const extra = parseTrustDialog(
    text.replace(`  ${TRUST_YES}`, `  ${TRUST_YES}\n  Maybe later`),
  );
  assert.ok(extra && extra.kind === "dialog");
  assert.equal(extra.options.length, 3);
});

test("stripAnsi removes color and attribute sequences", () => {
  assert.equal(
    stripAnsi(`\u001b[38;2;1;2;3mred${RESET} ${DIM}dim${RESET}`),
    "red dim",
  );
});

test("escape sequences other than plain CSI are removed and colon sub-parameters do not upset the dim reading", () => {
  const ESC = "\u001b";
  assert.equal(
    stripAnsi(
      `${ESC}]0;title\u0007a${ESC}[38:2::1:2:3mb${ESC}[4:3mc${ESC}[?25hd${ESC}]8;;http://x${ESC}\\e${ESC}=f`,
    ),
    "abcdef",
  );
  const rule = "─".repeat(20);
  const screen = [
    rule,
    `❯\u00a0${ESC}[0m${ESC}[2m${ESC}[38:2::9:9:9mTry it${ESC}[0m`,
    rule,
  ].join("\n");
  assert.equal(extractInputLine("claude", screen), "");
  const typed = [
    rule,
    `❯\u00a0${ESC}]0;t\u0007${ESC}[4:3mhello${ESC}[0m`,
    rule,
  ].join("\n");
  assert.equal(extractInputLine("claude", typed), "hello");
});

test("an input box whose two rule lines differ in width is not read", () => {
  const long = "─".repeat(40);
  const typedRule = "─".repeat(12);
  assert.equal(
    extractInputLine(
      "claude",
      [long, `❯\u00a0text`, typedRule, "  footer"].join("\n"),
    ),
    undefined,
  );
  assert.equal(
    extractInputLine(
      "claude",
      [long, `❯\u00a0text`, long, "  footer"].join("\n"),
    ),
    "text",
  );
});

test("a carriage return inside a line is a line break, so an overwritten redraw cannot hide text", () => {
  const rule = "─".repeat(20);
  assert.notEqual(
    extractInputLine(
      "claude",
      [rule, "❯\u00a0old text\r❯\u00a0", rule].join("\n"),
    ),
    "",
  );
  assert.equal(freshPromptReady("stale\r❯ typed\r❯"), true);
  assert.equal(freshPromptReady("❯ typed\rstale"), false);
});

test("dim continuation text is not typed input and leftover control characters make a screen unreadable", () => {
  const ESC = "\u001b";
  const rule = "─".repeat(20);
  const dimHint = `  ${ESC}[2mpress tab to accept${ESC}[0m`;
  assert.equal(
    extractInputLine(
      "claude",
      [rule, `❯\u00a0${ESC}[2mTry it${ESC}[0m`, dimHint, rule].join("\n"),
    ),
    "",
  );
  assert.equal(
    extractInputLine(
      "claude",
      [rule, `❯\u00a0`, "  real text", rule].join("\n"),
    ),
    "\n  real text",
  );
  for (const junk of [`${ESC}`, "\u009b", "\u0000", "\u0007", `${ESC}]0;cut`])
    assert.equal(
      extractInputLine("claude", [rule, `❯\u00a0text${junk}`, rule].join("\n")),
      undefined,
      JSON.stringify(junk),
    );
  assert.equal(extractInputLine("shell", `out\n❯ abc\u0007`), undefined);
  assert.equal(freshPromptReady("x\n❯\u0007"), false);
  assert.equal(stripAnsi("a\u009b31mb"), "ab");
});

test("the real Codex idle screens read as empty and the real typed screen as its text", () => {
  assert.equal(extractInputLine("codex", fixture("codex-idle-empty.ansi")), "");
  assert.equal(
    extractInputLine("codex", fixture("codex-idle-typed.ansi")),
    "hello typed",
  );
});

test("Codex typed text on more than one line is read whole, and a screen without a status line is unreadable", () => {
  const typed = fixture("codex-idle-typed.ansi").trimEnd().split("\n");
  const marker = typed.findIndex((line) => line.includes("hello typed"));
  typed.splice(marker + 1, 0, "  second line");
  assert.equal(
    extractInputLine("codex", typed.join("\n")),
    "hello typed\nsecond line",
  );
  assert.equal(
    extractInputLine("codex", typed.slice(0, marker + 2).join("\n")),
    undefined,
  );
  assert.equal(
    extractInputLine("codex", fixture("claude-idle-empty.ansi")),
    undefined,
  );
});

test("the real OMP idle screens read as empty and the real typed screen as its text", () => {
  assert.equal(extractInputLine("omp", fixture("omp-idle-empty.ansi")), "");
  assert.equal(
    extractInputLine("omp", fixture("omp-idle-typed.ansi")),
    "hello typed",
  );
  assert.equal(
    extractInputLine("omp", fixture("claude-idle-empty.ansi")),
    undefined,
  );
  assert.equal(extractInputLine("omp", "\u001b[2Jstuff\u0007"), undefined);
});

test("the real Codex trust dialog is parsed with its path, options, selection and last line", () => {
  const dialog = parseCodexTrustDialog(
    stripAnsi(fixture("codex-trust-dialog.ansi")),
  );
  assert.ok(dialog && dialog.kind === "dialog");
  assert.equal(dialog.path, "/tmp/s7q");
  assert.deepEqual(
    dialog.options.map((option) => option.text),
    [CODEX_TRUST_YES, CODEX_TRUST_NO],
  );
  assert.equal(dialog.selectedIndex, 0);
  assert.equal(dialog.confirmIsLastLine, true);
  assert.equal(
    parseCodexTrustDialog(stripAnsi(fixture("codex-idle-empty.ansi"))),
    undefined,
  );
});

test("the trust dialog parser and texts follow the host kind", () => {
  const plain = stripAnsi(fixture("codex-trust-dialog.ansi"));
  assert.ok(parseTrustDialogOf("codex", plain));
  assert.equal(parseTrustDialogOf("claude", plain), undefined);
  assert.equal(parseTrustDialogOf("omp", plain), undefined);
  assert.deepEqual(trustTexts("claude"), { yes: TRUST_YES, no: TRUST_NO });
  assert.deepEqual(trustTexts("codex"), {
    yes: CODEX_TRUST_YES,
    no: CODEX_TRUST_NO,
  });
  assert.equal(trustTexts("omp"), undefined);
});

test("the Codex trust dialog is not read as typed input, and a background colour 38 does not end the input", () => {
  assert.equal(
    extractInputLine("codex", fixture("codex-trust-dialog.ansi")),
    undefined,
  );
  const typed = fixture("codex-idle-typed.ansi").split("\n");
  const marker = typed.findIndex((line) => line.includes("hello typed"));
  typed.splice(marker + 1, 0, "\u001b[48;5;38m  second\u001b[0m");
  assert.equal(
    extractInputLine("codex", typed.join("\n")),
    "hello typed\nsecond",
  );
});

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  RELAY_PROMPT_MAX_BYTES,
  promptHash,
  relayTextProblem,
  type RelayOption,
} from "../src/herdr/prompt-relay.js";
import {
  TRUST_NO,
  TRUST_YES,
  CODEX_TRUST_NO,
  CODEX_TRUST_YES,
  extractInputLine,
  freshPromptReady,
  parseCodexTrustDialog,
  parseHostPrompt,
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
  assert.equal(
    extractInputLine(
      "codex",
      fixture("codex-idle-typed.ansi").replace("hello typed", "1. fix it"),
    ),
    "1. fix it",
  );
  const typed = fixture("codex-idle-typed.ansi").split("\n");
  const marker = typed.findIndex((line) => line.includes("hello typed"));
  typed.splice(marker + 1, 0, "\u001b[48;5;38m  second\u001b[0m");
  assert.equal(
    extractInputLine("codex", typed.join("\n")),
    "hello typed\nsecond",
  );
});

test("the Codex 0.160.0 screens read as empty, typed text, two typed lines and empty again after Ctrl+U", () => {
  assert.equal(
    extractInputLine("codex", fixture("codex-0160-idle-empty.ansi")),
    "",
  );
  assert.equal(
    extractInputLine("codex", fixture("codex-0160-idle-typed.ansi")),
    "hello typed",
  );
  assert.equal(
    extractInputLine("codex", fixture("codex-0160-idle-multiline.ansi")),
    "hello typed\nsecond line",
  );
  assert.equal(
    extractInputLine("codex", fixture("codex-0160-after-ctrl-u.ansi")),
    "",
  );
});

test("the OMP 18.3.1 screens read as empty, typed text and empty again after Ctrl+U", () => {
  assert.equal(
    extractInputLine("omp", fixture("omp-1831-idle-empty.ansi")),
    "",
  );
  assert.equal(
    extractInputLine("omp", fixture("omp-1831-idle-typed.ansi")),
    "hello typed",
  );
  assert.equal(
    extractInputLine("omp", fixture("omp-1831-after-ctrl-u.ansi")),
    "",
  );
});

test("the Codex 0.160.0 trust dialog is parsed, and no 0.160.0 idle screen is taken for it", () => {
  const dialog = parseCodexTrustDialog(
    stripAnsi(fixture("codex-0160-trust-dialog.ansi")),
  );
  assert.ok(dialog && dialog.kind === "dialog");
  assert.match(dialog.path, /^\/tmp\/p65-\w+$/);
  assert.deepEqual(
    dialog.options.map((option) => option.text),
    [CODEX_TRUST_YES, CODEX_TRUST_NO],
  );
  assert.equal(dialog.selectedIndex, 0);
  assert.equal(dialog.confirmIsLastLine, true);
  assert.equal(
    extractInputLine("codex", fixture("codex-0160-trust-dialog.ansi")),
    undefined,
  );
  for (const name of ["idle-empty", "idle-typed", "idle-multiline"])
    assert.equal(
      parseCodexTrustDialog(stripAnsi(fixture(`codex-0160-${name}.ansi`))),
      undefined,
      name,
    );
});

const prompt = (name: string): string =>
  readFileSync(path.resolve("test/fixtures/prompts", name), "utf8");

const option = (
  number: number,
  text: string,
  acceptsText = false,
  widensPermissions = false,
): RelayOption => ({ number, text, acceptsText, widensPermissions });

test("the real Claude Bash permission prompt parses to its exact text and numbered options; only wording a fixture proves opens a text field or widens permissions", () => {
  const parsed = parseHostPrompt(
    "claude",
    prompt("claude-bash-permission.ansi"),
  )!;
  assert.equal(parsed.selectedIndex, 0);
  assert.equal(
    parsed.text,
    [
      " Bash command",
      ' Tip: auto mode handles these prompts for you — choose "switch to auto mode" below',
      " Create empty file spike-a.txt",
      "╌".repeat(parsed.text.split("\n")[3]!.length),
      " touch spike-a.txt",
      "╌".repeat(parsed.text.split("\n")[3]!.length),
      " Do you want to proceed?",
    ].join("\n"),
  );
  assert.deepEqual(parsed.options, [
    option(1, "Yes"),
    option(
      2,
      "Yes, and always allow access to /tmp/cph-spike/repo from this project",
      false,
      true,
    ),
    option(
      3,
      "Yes, and switch to auto mode · auto mode handles these prompts for you",
      false,
      true,
    ),
    option(4, "No", true),
  ]);
});

test("the real Claude Write permission prompt parses; its Yes opens a text field and switching to accept edits widens permissions", () => {
  const parsed = parseHostPrompt(
    "claude",
    prompt("claude-write-permission.ansi"),
  )!;
  assert.equal(parsed.selectedIndex, 0);
  assert.ok(parsed.text.startsWith(" Create file\n spike-b.txt\n"));
  assert.ok(parsed.text.endsWith(" Do you want to create spike-b.txt?"));
  assert.deepEqual(parsed.options, [
    option(1, "Yes", true),
    option(
      2,
      "Yes, and switch to accept edits (auto-approve file edits and common file commands) for this session (shift+tab)",
      false,
      true,
    ),
    option(3, "No"),
  ]);
});

test("the real screens with an open text field parse with the field's wording and the selection kept", () => {
  const bash = parseHostPrompt(
    "claude",
    prompt("claude-bash-permission-no-textfield.ansi"),
  )!;
  assert.equal(bash.selectedIndex, 3);
  assert.equal(
    bash.options[3]!.text,
    "No, and tell Claude what to do differently",
  );
  const write = parseHostPrompt(
    "claude",
    prompt("claude-write-permission-yes-textfield.ansi"),
  )!;
  assert.equal(write.selectedIndex, 0);
  assert.equal(write.options[0]!.text, "Yes, and tell Claude what to do next");
  assert.equal(
    bash.text,
    parseHostPrompt("claude", prompt("claude-bash-permission.ansi"))!.text,
  );
});

test("wording no fixture proves is flagged by the heuristic only when it adds a rule or switches the mode", () => {
  const screen = prompt("claude-bash-permission.ansi")
    .replace(
      "4. \u001b[0mNo",
      "4. \u001b[0mYes, and don't ask again for touch commands",
    )
    .replace(
      "1. \u001b[0m\u001b[38;2;177;185;249mYes",
      "1. \u001b[0m\u001b[38;2;177;185;249mYes, once",
    );
  const parsed = parseHostPrompt("claude", screen)!;
  assert.deepEqual(
    parsed.options.map((entry) => [
      entry.text.slice(0, 23),
      entry.widensPermissions,
    ]),
    [
      ["Yes, once", false],
      ["Yes, and always allow a", true],
      ["Yes, and switch to auto", true],
      ["Yes, and don't ask agai", true],
    ],
  );
  assert.equal(
    parsed.options.some((entry) => entry.acceptsText),
    false,
  );
});

test("parseHostPrompt returns undefined for dialogs that are not proven: not last, fake above an input line, two markers, control characters, oversized text, wrapped options", () => {
  for (const name of [
    "synthetic-dialog-not-last.ansi",
    "synthetic-fake-above-input.ansi",
    "synthetic-two-selected.ansi",
    "synthetic-control-char.ansi",
    "synthetic-oversized.ansi",
    "synthetic-wrapped-option.ansi",
  ])
    assert.equal(parseHostPrompt("claude", prompt(name)), undefined, name);
  assert.ok(
    Buffer.byteLength(prompt("synthetic-oversized.ansi")) >
      RELAY_PROMPT_MAX_BYTES,
  );
});

test("parseHostPrompt returns undefined for every other host and for ordinary screens", () => {
  const real = prompt("claude-bash-permission.ansi");
  for (const kind of ["codex", "omp", "shell", "unknown"])
    assert.equal(parseHostPrompt(kind, real), undefined, kind);
  for (const name of [
    "codex-0160-trust-dialog.ansi",
    "codex-trust-dialog.ansi",
    "codex-0160-idle-typed.ansi",
    "omp-1831-idle-empty.ansi",
    "omp-idle-typed.ansi",
    "claude-idle-empty.ansi",
    "claude-idle-typed.ansi",
  ])
    for (const kind of ["claude", "codex", "omp"])
      assert.equal(parseHostPrompt(kind, fixture(name)), undefined, name);
  assert.equal(
    parseHostPrompt("claude", fixture("claude-trust-dialog.txt")),
    undefined,
  );
});

test("promptHash is 64 lowercase hex, stable, and changes with the agent, pane, host, text, an option's text or the option order", () => {
  const base = {
    agentId: "dev-g1",
    paneId: "w1:p1",
    hostKind: "claude",
    text: "Do you want to proceed?",
    options: [option(1, "Yes"), option(2, "No", true)],
  };
  const reference = promptHash(base);
  assert.match(reference, /^[0-9a-f]{64}$/);
  assert.equal(promptHash({ ...base, options: [...base.options] }), reference);
  const variants = [
    { ...base, agentId: "dev-g2" },
    { ...base, paneId: "w1:p2" },
    { ...base, hostKind: "codex" },
    { ...base, text: "Do you want to proceed!" },
    { ...base, options: [option(1, "Yes!"), option(2, "No", true)] },
    { ...base, options: [option(1, "No", true), option(2, "Yes")] },
    { ...base, options: [option(1, "Yes"), option(2, "No", false)] },
    {
      ...base,
      options: [option(1, "Yes", false, true), option(2, "No", true)],
    },
  ];
  for (const variant of variants)
    assert.notEqual(promptHash(variant), reference);
});

test("relayTextProblem accepts plain text and refuses newlines, control or format characters, a leading tab or command character, over 1000 bytes and ill-formed UTF-16", () => {
  for (const ok of ["use ls instead", "do not touch files ✓", "a".repeat(1000)])
    assert.equal(relayTextProblem(ok), undefined, ok);
  for (const bad of [
    "",
    "   ",
    "two\nlines",
    "bell\u0007",
    "zero\u200bwidth",
    "\ttab",
    "/clear",
    "!ls",
    "#note",
    "?help",
    "@file",
    " /clear",
    "a".repeat(1001),
    "é".repeat(501),
    "lone\ud800surrogate",
  ])
    assert.notEqual(relayTextProblem(bad), undefined, JSON.stringify(bad));
});

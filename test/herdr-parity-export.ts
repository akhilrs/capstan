/**
 * Writes the fixtures capstan-herdr (rust/crates/herdr) is tested against: what the Node screen parsers, prompt relay
 * checks, argument builders, runner helpers, process-activity parsers and the Herdr adapter compute on fixed inputs
 * (every test/fixtures capture, recorded /proc and ps samples, scripted runner answers). The adapter file holds the
 * herdr argv/stdin call sequences the Node adapter makes against the fake Herdr, with the answers it got, so the Rust
 * adapter can be driven by the same answers and must make the same calls and return the same results.
 *
 * Run `npm run build && node dist/test/herdr-parity-export.js` after an intended change and commit the result;
 * herdr-parity.test.ts fails while the committed files differ from a fresh export.
 */
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import {
  HerdrAdapter,
  buildAgentEnvironment,
  claudeArguments,
  isAgentName,
  shellQuote,
  type ClaudeRoleSettings,
} from "../src/herdr/adapter.js";
import {
  COMMAND_START,
  BRANCH_PATTERN,
  ENVIRONMENT_KEY,
  PANE_PATTERN,
  SIMPLE_VALUE,
  TAB_PATTERN,
  WORKSPACE_PATTERN,
  isSafeText,
  requireLabel,
} from "../src/herdr/adapter-validate.js";
import {
  codexArguments,
  ompArguments,
  tomlString,
} from "../src/herdr/hosts.js";
import {
  herdrAgentName,
  projectDisplayName,
  projectSlug,
  workspaceLabel,
} from "../src/herdr/naming.js";
import {
  ProcessActivityTracker,
  ToolProcessWalker,
  HerdrProcessProbe,
  parseProcStat,
  parsePsTable,
  parsePsTime,
  toolProcesses,
  type ProcIo,
  type ProcessEntry,
} from "../src/herdr/process-activity.js";
import {
  checkAnswer,
  promptHash,
  relayTextProblem,
  type CapturedPrompt,
  type PromptAnswer,
  type RelayOption,
} from "../src/herdr/prompt-relay.js";
import {
  describeOutput,
  failureOf,
  herdrEnvironment,
  runJson,
  type HerdrResult,
} from "../src/herdr/runner.js";
import {
  classifyInputBlocker,
  extractInputLine,
  freshPromptReady,
  parseBlockingDialog,
  parseHostPrompt,
  parseTrustDialogOf,
  promptFooter,
  stripAnsi,
  TEXT_FIELD_WORDING,
  TRUST_NO,
  TRUST_YES,
} from "../src/herdr/screen.js";
import {
  CLEAN_ENV,
  FakeHerdr,
  NBSP,
  SHELL_READY,
  idleScreen,
  type FakePane,
} from "./herdr-adapter-harness.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const PARITY_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "herdr",
  "tests",
  "parity",
);
const FIXTURES = path.join(root, "test", "fixtures");

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

const text = (name: string): string =>
  readFileSync(path.join(FIXTURES, name), "utf8");
const sha = (value: string): string =>
  createHash("sha256").update(value, "utf8").digest("hex");
const orNull = <T>(value: T | undefined): T | null =>
  value === undefined ? null : value;

/** `{ok}` or `{error: message}` for a call that may throw. */
function attempt(action: () => unknown): Json {
  try {
    return { ok: (action() ?? null) as Json };
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) };
  }
}

// ------------------------------------------------------------------------------------------------------------- screen

const RULE = "─".repeat(40);
const DIM = "\u001b[2m";
const RESET = "\u001b[0m";

function claudeScreen(...region: string[]): string {
  return ["header art", "", RULE, ...region, RULE, "  footer"].join("\r\n");
}

/** Screens that are not captures but exercise the corners of the parsers. */
const SYNTHETIC_SCREENS: Array<[string, string]> = [
  ["empty", ""],
  ["blank-lines", "\n\n  \n"],
  ["shell-ready", "user in dir\r\n❯ "],
  ["shell-typed", "user in dir\r\n❯ ls -la  "],
  ["shell-nbsp", `user in dir\r\n❯${NBSP}pwd`],
  ["shell-bare", "x\r\n❯"],
  ["shell-dollar", "x\r\n$"],
  ["shell-control", "x\r\n❯ a\u0007b"],
  ["shell-terminator", "x\r\n❯ a\u2028b"],
  ["shell-text-after", "x\r\n❯ ls\r\nmore output"],
  ["claude-empty", claudeScreen(`❯${NBSP}${RESET}${DIM}Try "x"${RESET}`)],
  ["claude-dim-no-reset", claudeScreen(`❯${NBSP}${DIM}Try "x"${RESET}`)],
  ["claude-typed", claudeScreen(`❯${NBSP}hello  `)],
  ["claude-bare-marker", claudeScreen("❯")],
  ["claude-space-marker", claudeScreen("❯ typed")],
  ["claude-two-lines", claudeScreen(`❯${NBSP}one`, "  two", "  three  ")],
  [
    "claude-dim-continuation",
    claudeScreen(`❯${NBSP}one`, `${DIM}  hint${RESET}`, "  real"),
  ],
  [
    "claude-colour-2",
    claudeScreen(
      `\u001b[38;2;1;2;3m❯${RESET}${NBSP}\u001b[38;5;2mgreen${RESET}`,
    ),
  ],
  [
    "claude-colour-colon",
    claudeScreen(`❯${NBSP}\u001b[38:2::1:2:3mtext${RESET}`),
  ],
  [
    "claude-dim-then-normal",
    claudeScreen(`❯${NBSP}${DIM}ghost${RESET}${NBSP}typed`),
  ],
  ["claude-one-rule", ["x", RULE, "❯ y"].join("\n")],
  ["claude-rule-lengths", ["x", RULE, "❯ y", "─".repeat(39)].join("\n")],
  ["claude-no-gap", [RULE, RULE].join("\n")],
  ["claude-no-marker", claudeScreen("not a prompt")],
  ["claude-control", claudeScreen("❯ a\u0001b")],
  ["claude-cr-only", ["a", RULE, "❯ hi", RULE, "f"].join("\r")],
  ["claude-lone-escape", claudeScreen("❯ a\u001bb")],
  ["claude-csi-unterminated", claudeScreen("❯ a\u001b[31")],
  ["claude-osc", claudeScreen("❯ \u001b]0;title\u0007typed")],
  ["claude-c1-csi", claudeScreen("❯ \u009b1mtyped")],
  [
    "codex-empty",
    [
      "x",
      "› \u001b[2mAsk anything\u001b[0m",
      "",
      "\u001b[38;5;244m  gpt  ·  ~/dir\u001b[0m",
    ].join("\r\n"),
  ],
  [
    "codex-typed",
    ["x", "› hello", "  more", "\u001b[38;5;244m  gpt\u001b[0m"].join("\r\n"),
  ],
  ["codex-no-status", ["x", "› hello", "  more"].join("\r\n")],
  [
    "codex-bg-colour-only",
    ["x", "› hi", "\u001b[48;5;1m  gpt\u001b[0m"].join("\r\n"),
  ],
  ["codex-bare", ["x", "›", "\u001b[38;2;1;2;3m  gpt\u001b[0m"].join("\r\n")],
  [
    "codex-dialog",
    ["Folder access", "/x", "enter continue · esc quit"].join("\r\n"),
  ],
  ["omp-empty", "output\r\n\r\n"],
  ["omp-typed", "output\r\n╰─ build it  "],
  ["omp-bare", "output\r\n╰─"],
  ["omp-control", "output\r\n╰─ a\u0007"],
  ["omp-other", "output\r\nplain"],
  ["unicode-js-space", `x\r\n❯\u2003typed`],
  [
    "trust-claude-ok",
    [
      "Accessing workspace:",
      "",
      " /tmp/work",
      "",
      " ❯ 1. Yes, I trust this folder",
      "   2. No, exit",
      "",
      " Enter to confirm · Esc to cancel",
    ].join("\n"),
  ],
  [
    "trust-claude-two-selected",
    [
      "Accessing workspace:",
      "/tmp/work",
      "",
      "❯ Yes, I trust this folder",
      "❯ No, exit",
      "",
      "Enter to confirm",
    ].join("\n"),
  ],
  [
    "trust-claude-not-last",
    [
      "Accessing workspace:",
      "/tmp/work",
      "",
      "  Yes, I trust this folder",
      "❯ No, exit",
      "",
      "Enter to confirm",
      "more",
    ].join("\n"),
  ],
  [
    "trust-claude-wrapped",
    [
      "Accessing workspace:",
      "/tmp/very",
      "long path",
      "",
      "Enter to confirm",
    ].join("\n"),
  ],
  ["trust-claude-no-confirm", ["Accessing workspace:", "/tmp/x"].join("\n")],
  ["trust-claude-title-only", "Accessing workspace:"],
  [
    "trust-codex-ok",
    [
      "Folder access",
      "/tmp/work",
      "Do you trust?",
      "› 1. Trust and continue",
      "  2. Quit",
      "",
      "enter continue · esc quit",
    ].join("\n"),
  ],
  [
    "trust-codex-nbsp-number",
    [
      "Folder access",
      "/tmp/work",
      "",
      "›\u00a01.\u00a0Trust and continue",
      "2.   Quit",
      "3.",
      "x. nope",
      "enter continue",
    ].join("\n"),
  ],
  [
    "prompt-two-hanging",
    [
      "earlier",
      "─".repeat(30),
      " Bash command",
      "   touch a",
      " Do you want to proceed?",
      " ❯ 1. Yes",
      "   2. No",
      "",
      " Esc to cancel · Tab to amend",
    ].join("\r\n"),
  ],
  [
    "prompt-create",
    [
      "─".repeat(30),
      " Create file",
      " Do you want to create a.txt?",
      " ❯ 1. Yes",
      "   2. Yes, and switch to accept edits",
      "   3. No",
      " Esc to cancel",
    ].join("\r\n"),
  ],
  [
    "prompt-leading-zero",
    [
      "─".repeat(30),
      " Header",
      " Do you want to proceed?",
      " ❯ 01. Yes",
      "   2. No",
      " Esc to cancel",
    ].join("\n"),
  ],
  [
    "prompt-heuristic-wording",
    [
      "─".repeat(30),
      " Header",
      " Do you want to proceed?",
      " ❯ 1. Don't ask again for this tool",
      "   2. PERMANENTLY allow it",
      "   3. Ｓwitch to other",
      "   4. contrarily switch tomorrow",
      " Esc to cancel",
    ].join("\n"),
  ],
  [
    "prompt-no-rule-blank-gap",
    [
      "x",
      "",
      "",
      " Header",
      " Do you want to proceed?",
      " ❯ 1. Yes",
      "   2. No",
      " Esc to cancel",
    ].join("\n"),
  ],
  [
    "prompt-wrapped-ok",
    [
      "─".repeat(30),
      " Header",
      " Do you want to proceed?",
      " ❯ 1. Yes",
      "   2. Yes, and always allow access to /a/very/long",
      "      path/here from this project",
      "   3. No",
      " Esc to cancel",
    ].join("\n"),
  ],
  [
    "prompt-wrapped-bad",
    [
      "─".repeat(30),
      " Header",
      " Do you want to proceed?",
      "      stray hanging",
      " ❯ 1. Yes",
      "   2. No",
      " Esc to cancel",
    ].join("\n"),
  ],
  [
    "prompt-terminator-option",
    [
      "─".repeat(30),
      " Header",
      " Do you want to proceed?",
      " ❯ 1. Yes\u2028and more",
      "   2. No",
      " Esc to cancel",
    ].join("\n"),
  ],
  [
    "dialog-plain",
    [
      "─".repeat(30),
      " Settings",
      "   something here",
      "",
      " Enter to continue · Esc to cancel",
    ].join("\n"),
  ],
  [
    "dialog-numbered",
    ["─".repeat(30), " Settings", "  1. a", "  2. b", " Esc to cancel"].join(
      "\n",
    ),
  ],
  [
    "dialog-numbered-marker-space",
    ["─".repeat(30), " Settings", " ❯ \u20031.\u2003a", " Esc to cancel"].join(
      "\n",
    ),
  ],
  ["dialog-no-rule", ["Settings", "text", "Esc to cancel"].join("\n")],
];

const KINDS = ["claude", "codex", "omp", "shell", "other"];
const HASH_PANE = { agentId: "dev", paneId: "w1:p1", hostKind: "claude" };

function optionsJson(options: readonly RelayOption[]): Json {
  return options.map((option) => ({ ...option }));
}

function screenView(content: string): Json {
  const plain = stripAnsi(content);
  const host = parseHostPrompt("claude", content);
  const dialog = parseBlockingDialog("claude", content);
  const trust = (kind: string): Json => {
    const parsed = parseTrustDialogOf(kind, content);
    if (parsed === undefined) return null;
    if (parsed.kind === "wrapped_path") return { kind: "wrapped_path" };
    return {
      kind: "dialog",
      path: parsed.path,
      options: parsed.options.map((option) => ({ ...option })),
      selectedIndex: orNull(parsed.selectedIndex),
      confirmIsLastLine: parsed.confirmIsLastLine,
    };
  };
  return {
    bytes: Buffer.byteLength(content, "utf8"),
    sha: sha(content),
    stripped: { length: plain.length, sha: sha(plain) },
    fresh: freshPromptReady(plain),
    freshRaw: freshPromptReady(content),
    input: Object.fromEntries(
      KINDS.map((kind) => [kind, orNull(extractInputLine(kind, content))]),
    ),
    blocker: Object.fromEntries(
      KINDS.map((kind) => [kind, classifyInputBlocker(kind, content)]),
    ),
    trust: Object.fromEntries(
      ["claude", "codex", "omp"].map((kind) => [kind, trust(kind)]),
    ),
    footer: orNull(promptFooter(content)),
    hostPrompt:
      host === undefined
        ? null
        : {
            text: host.text,
            selectedIndex: host.selectedIndex,
            options: optionsJson(host.options),
            sha: promptHash({
              ...HASH_PANE,
              text: host.text,
              options: host.options,
            }),
          },
    hostPromptCodex: parseHostPrompt("codex", content) === undefined ? null : 1,
    dialog:
      dialog === undefined
        ? null
        : {
            text: dialog.text,
            sha: promptHash({
              ...HASH_PANE,
              text: dialog.text,
              options: [],
              dialog: true,
            }),
          },
  };
}

function fixtureNames(): string[] {
  const top = readdirSync(FIXTURES)
    .filter((name) => /\.(?:ansi|txt)$/.test(name))
    .sort();
  const prompts = readdirSync(path.join(FIXTURES, "prompts"))
    .filter((name) => name.endsWith(".ansi"))
    .sort()
    .map((name) => `prompts/${name}`);
  return [...top, ...prompts];
}

function screenCases(): Json {
  return {
    fixtures: fixtureNames().map((name) => ({
      name,
      ...(screenView(text(name)) as { [key: string]: Json }),
    })),
    synthetic: SYNTHETIC_SCREENS.map(([name, content]) => ({
      name,
      content,
      ...(screenView(content) as { [key: string]: Json }),
    })),
    textFieldWording: Object.entries(TEXT_FIELD_WORDING),
  };
}

// ------------------------------------------------------------------------------------------------------------- relay

// prettier-ignore
// prettier-ignore
const RELAY_TEXTS = [
  "", " ", "ok", "\tx", " /cmd", "!x", "#x", "?x", "@x", "  \u2003@x", "a\nb", "a\u0000b", "a\u200eb",
  "a\u2028b", "a\ufffeb", "a\u{10ffff}b", "x".repeat(1000), "x".repeat(1001), "é".repeat(500), "é".repeat(501),
  "日本語 text — ok", "\u00a0", "\ufeff", "tab\tinside", "joiner\u200dinside",
];

function relayCases(): Json {
  const base = parseHostPrompt(
    "claude",
    readFileSync(
      path.join(FIXTURES, "prompts", "claude-bash-permission.ansi"),
      "utf8",
    ),
  )!;
  const prompt: CapturedPrompt = {
    ...HASH_PANE,
    text: base.text,
    options: base.options,
    promptSha: promptHash({
      ...HASH_PANE,
      text: base.text,
      options: base.options,
    }),
  };
  const answers: PromptAnswer[] = [
    { kind: "esc" },
    { kind: "option", number: 1 },
    { kind: "option", number: 4 },
    { kind: "option", number: 5 },
    { kind: "option", number: 0 },
    { kind: "text", number: 4, text: "fine" },
    { kind: "text", number: 4, text: "" },
    { kind: "text", number: 4, text: "/slash" },
    { kind: "text", number: 1, text: "fine" },
    { kind: "text", number: 9, text: "fine" },
  ];
  return {
    texts: RELAY_TEXTS.map((value) => ({
      text: value,
      problem: orNull(relayTextProblem(value)),
    })),
    prompt: { text: prompt.text, options: optionsJson(prompt.options) },
    promptSha: prompt.promptSha,
    answers: answers.map((answer) => ({
      answer: answer as unknown as Json,
      refusal: orNull(checkAnswer(prompt, answer)),
    })),
    hashes: [
      [{ ...HASH_PANE, text: "t", options: [] }],
      [{ ...HASH_PANE, text: "t", options: [], dialog: true }],
      [{ ...HASH_PANE, text: 'a\u2028"b\\', options: base.options }],
      [{ ...HASH_PANE, text: "é\u{1f600}\u007f", options: [] }],
    ].map(([input]) => ({
      input: {
        ...input!,
        options: optionsJson(input!.options),
      } as Json,
      sha: promptHash(input!),
    })),
  };
}

// ---------------------------------------------------------------------------------------------------------- validate

// prettier-ignore
const VALUE_TEXTS = [
  "", " ", "a", "dev", "dev-g1", "-dev", "a.b_c-d", "a".repeat(64), "a".repeat(65), "w1", "w1:p1",
  "w1:t1", "w1:p", "w1:p1\n", "x1:p1", "wA9:pB2", "cap/task/dev-g1", "cap//x", "/abs", "a b",
  "a@b%c+d=e:f,g.h/i-j_k", "É", "name\u0000", "FOO_BAR", "foo", "_X9", "9X", "a1_b2",
];

// prettier-ignore
// prettier-ignore
const LABEL_TEXTS = [
  "dev", "", "  ", "a\nb", "x".repeat(64), "x".repeat(65), "é".repeat(64), "é".repeat(65),
  "\u{1f600}".repeat(64), "\u{1f600}".repeat(65), "a\u200eb", "a\tb", "proj · part",
];

// prettier-ignore
// prettier-ignore
const SAFE_TEXTS = [
  "hello", "", " \n ", "line1\nline2", "tab\there", "a\u200cb\u200dc", "a\u0007b", "a\u200eb", "a\u2028b",
  "a\r\nb", "/cmd", "\tstart", "  !bang", "ok ? fine", "é".repeat(10),
];

function role(overrides: Partial<ClaudeRoleSettings> = {}): ClaudeRoleSettings {
  return {
    model: null,
    permissionMode: "default",
    allow: [],
    deny: [],
    hooks: "inherit",
    ...overrides,
  } as ClaudeRoleSettings;
}

function validateCases(): Json {
  const patterns: Record<string, RegExp> = {
    branch: BRANCH_PATTERN,
    workspace: WORKSPACE_PATTERN,
    pane: PANE_PATTERN,
    tab: TAB_PATTERN,
    simple: SIMPLE_VALUE,
    envKey: ENVIRONMENT_KEY,
    token: /^[a-z][a-z0-9_]{0,31}$/,
  };
  const claude: Array<[string, ClaudeRoleSettings, string | undefined]> = [
    ["plain", role(), undefined],
    ["prompt", role(), "/tmp/p.md"],
    ["hooks-off", role({ hooks: "off" }), undefined],
    ["model", role({ model: "opus" }), undefined],
    [
      "tools",
      role({ allow: ["Read", "Bash(ls:*)"], deny: ["WebFetch"] }),
      "/tmp/p.md",
    ],
    [
      "mcp",
      role({
        mcp: [
          { name: "a", command: "node", args: ["x.js", "é\u{1f600}"] },
          { name: "b", command: "py", args: [] },
        ],
      }),
      undefined,
    ],
    ["dash-model", role({ model: "-x" }), undefined],
    ["dash-allow", role({ allow: ["-x"] }), undefined],
    ["control-allow", role({ allow: ["a\nb"] }), undefined],
    ["empty-allow", role({ allow: [""] }), undefined],
    [
      "control-mcp",
      role({ mcp: [{ name: "a", command: "n", args: ["x\u0007"] }] }),
      undefined,
    ],
    [
      "dash-mcp",
      role({ mcp: [{ name: "a", command: "-n", args: [] }] }),
      undefined,
    ],
    ["control-prompt", role(), "/tmp/a\nb"],
  ];
  const baseEnvironment = {
    PATH: "/usr/bin",
    HOME: "/home/u",
    USER: "u",
    TERM: "xterm",
    LANG: "",
    CAPSTAN_SECRET: "no",
    TOKEN_X: "tok",
    EMPTY_X: "",
    BAD_X: "a\nb",
  };
  const environments: Array<[string, Record<string, string>, string[]]> = [
    ["base", { FOO: "bar" }, []],
    ["pass", {}, ["TOKEN_X"]],
    ["pass-empty", {}, ["EMPTY_X"]],
    ["pass-bad", {}, ["BAD_X"]],
    ["pass-capstan", {}, ["CAPSTAN_SECRET"]],
    ["pass-lowercase", {}, ["lower"]],
    ["extras-override", { PATH: "/x", CAPSTAN_A: "1" }, []],
    ["extras-bad-key", { "bad-key": "1" }, []],
    ["extras-bad-value", { OK: "a\u2028b" }, []],
    ["extras-cr", { OK: "a\r" }, []],
  ];
  const worktrees: Array<string | undefined> = [undefined, "/"];
  return {
    patterns: Object.entries(patterns).map(([name, pattern]) => ({
      name,
      cases: VALUE_TEXTS.map((value) => [value, pattern.test(value)]),
    })),
    agentNames: VALUE_TEXTS.map((value) => [value, isAgentName(value)]),
    labels: LABEL_TEXTS.map((value) => ({
      value,
      result: attempt(() => requireLabel(value)),
    })),
    safeText: SAFE_TEXTS.map((value) => ({
      value,
      safe: isSafeText(value),
      command: COMMAND_START.test(value),
    })),
    shellQuote: ["", "a", "it's", "'", "a b", "é", "$(x)"].map((value) => [
      value,
      shellQuote(value),
    ]),
    slugs: [
      "capstan",
      "My Project",
      "9lives",
      "Ünïcödé Prøject",
      "---",
      "a--b--c",
      "x".repeat(30),
      "abcdefghi-jk",
      "",
      "日本語",
      "ﬁne ꜰont",
      "éclair",
    ].map((value) => [value, projectSlug(value)]),
    displayNames: [
      [null, "/home/u/proj"],
      ["  Named  ", "/home/u/proj"],
      ["", "/home/u/proj/"],
      ["a\u0007b\u0085c", "/x"],
      ["x".repeat(40), "/x"],
      ["\u{1f600}".repeat(30), "/x"],
      [null, "/"],
      ["   ", "///"],
      [null, "relative/dir"],
    ].map(([configured, rootPath]) => ({
      configured: configured ?? null,
      root: rootPath ?? "",
      name: projectDisplayName(configured ?? null, rootPath ?? ""),
    })),
    agentNamesFor: [
      ["acme", "dev-1"],
      ["acme", "Dev"],
      ["acme", "dev-g1-with-a-long-tail"],
      ["p", "x_y"],
    ].map(([slug, id]) => ({
      slug: slug!,
      id: id!,
      result: attempt(() => herdrAgentName(slug!, id!)),
    })),
    workspaceLabels: [
      ["proj", "pm"],
      ["x".repeat(60), "workers"],
      ["é", "\u{1f600}".repeat(70)],
    ].map(([name, part]) => [name!, part!, workspaceLabel(name!, part!)]),
    claude: claude.map(([name, settings, prompt]) => ({
      name,
      role: settings as unknown as Json,
      prompt: orNull(prompt),
      result: attempt(() => claudeArguments(settings, prompt)),
    })),
    environments: environments.map(([name, extras, pass]) => ({
      name,
      base: baseEnvironment,
      extras: Object.entries(extras),
      pass,
      result: attempt(() => {
        const built = buildAgentEnvironment(baseEnvironment, extras, pass);
        return Object.fromEntries(
          Object.entries(built).sort(([a], [b]) => (a < b ? -1 : 1)),
        );
      }),
    })),
    tomlStrings: [
      "plain",
      'q"uote',
      "back\\slash",
      "nl\n",
      "\u007fdel\u009f",
      "é\u{1f600}",
      "",
    ].map((value) => [value, tomlString(value)]),
    hosts: worktrees.flatMap((worktree) =>
      [null, "gpt", "-bad"].map((model) => ({
        worktree: orNull(worktree),
        model,
        prompt: 'say "hi"\nnow',
        codex: attempt(() =>
          codexArguments({ model }, 'say "hi"\nnow', worktree),
        ),
        omp: attempt(() => ompArguments({ model }, "/tmp/p.md")),
      })),
    ),
    hostsMissingWorktree: attempt(() =>
      codexArguments({ model: null }, "p", "/definitely/not/here"),
    ),
    hostsHuge: attempt(() =>
      codexArguments({ model: null }, "x".repeat(121 * 1024), undefined),
    ),
  };
}

// ------------------------------------------------------------------------------------------------------------ runner

async function runnerCases(): Promise<Json> {
  const results: Array<[string, HerdrResult]> = [
    ["ok", { code: 0, stdout: '{"id":"x","result":{"a":1}}', stderr: "" }],
    ["bom", { code: 0, stdout: '\ufeff{"result":{"a":1}}', stderr: "" }],
    ["empty-result", { code: 0, stdout: '{"result":{}}', stderr: "" }],
    ["no-result", { code: 0, stdout: '{"id":"x"}', stderr: "" }],
    ["array-result", { code: 0, stdout: '{"result":[1]}', stderr: "" }],
    ["null-result", { code: 0, stdout: '{"result":null}', stderr: "" }],
    ["array-body", { code: 0, stdout: "[1]", stderr: "" }],
    ["null-body", { code: 0, stdout: "null", stderr: "" }],
    ["number-body", { code: 0, stdout: "3", stderr: "" }],
    ["not-json", { code: 0, stdout: "hello\u0007\nworld", stderr: "" }],
    ["blank", { code: 0, stdout: "", stderr: "" }],
    [
      "error-in-stdout",
      {
        code: 0,
        stdout: '{"error":{"code":"nope","message":"a\\u0007b"}}',
        stderr: "",
      },
    ],
    [
      "error-code-bad",
      {
        code: 1,
        stdout: "",
        stderr: '{"error":{"code":"bad code!","message":"m"}}',
      },
    ],
    [
      "error-code-long",
      {
        code: 1,
        stdout: "",
        stderr: `{"error":{"code":"${"c".repeat(65)}","message":"m"}}`,
      },
    ],
    [
      "error-no-message",
      { code: 1, stdout: "", stderr: '{"error":{"code":"x"}}' },
    ],
    [
      "error-message-long",
      {
        code: 1,
        stdout: "",
        stderr: `{"error":{"code":"x","message":"${"é".repeat(300)}"}}`,
      },
    ],
    ["error-not-object", { code: 1, stdout: "", stderr: '{"error":"flat"}' }],
    [
      "error-in-stdout-exit",
      {
        code: 2,
        stdout: '{"error":{"code":"old","message":"older"}}',
        stderr: "",
      },
    ],
    [
      "exit-plain",
      { code: 3, stdout: "", stderr: "boom\u001b[31m red\u200b\n" },
    ],
    ["exit-silent", { code: 4, stdout: "", stderr: "" }],
    [
      "exit-bom",
      {
        code: 1,
        stdout: "",
        stderr: '\ufeff{"error":{"code":"b","message":"bom"}}',
      },
    ],
  ];
  return {
    describe: [
      "",
      "  ",
      "plain",
      "a\u0007b\u200bc\u0085d",
      "é".repeat(250),
      "\u{1f600}".repeat(250),
      "  padded  ",
      "\u00a0only\u00a0",
    ].map((value) => [value, describeOutput(value)]),
    results: await Promise.all(
      results.flatMap(([name, outcome]) =>
        [["pane", "get"], []].map(async (args) => {
          const error = failureOf(args, outcome);
          return {
            name,
            args,
            outcome: outcome as unknown as Json,
            failure: { code: error.code, message: error.message },
            json: await runJson(async () => outcome, args).then(
              (value) => ({ ok: value as Json }),
              (caught: unknown) => ({ error: errorJson(caught) }),
            ),
          };
        }),
      ),
    ),
    environment: herdrEnvironment({
      HERDR_SESSION: "x",
      HERDR_PANE: "y",
      HOME: "/h",
      herdr_lower: "z",
    }) as Json,
  };
}

// ----------------------------------------------------------------------------------------------------------- process

// prettier-ignore
const PROC_STATS: Array<[number, string]> = [
  [ 10, "10 (bash) S 1 10 10 34816 10 4194304 100 0 0 0 5 7 11 13 20 0 1 0 12345 1000 100 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 0 0 0 0 0 0", ],
  [ 11, "11 (my (weird) name) R 10 11 10 0 -1 4194304 1 0 0 0 100 200 0 0 20 0 1 0 99999 1 1 0", ],
  [12, "12 (a b) S 10 12 10 0 -1 0 0 0 0 0 1 1 0 0 20 0 1 0 5 0 0"],
  [13, "13 bash S 10"],
  [14, "14 (x) S 10 14"],
  [15, "15 (x) S abc 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1"],
  [16, "16 (x) S 1.5 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1"],
  [17, "17 (x) S 1e1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1"],
  [18, "18 (x) S 2 1 1 1 1 1 1 1 1 1 1 3 0x10 1 1 1 1 1 1 1 1 1"],
  [19, "19 (x) S 2 1 1 1 1 1 1 1 1 1 1 3 4 1 1 1 1 1 1 7 1 1"],
  [20, "20 (x)\tS\t2\t1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 7"],
  [21, ") (x) S 2 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 7 1 1"],
  [22, "22 (x) S 2 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 7 \n"],
  [23, "23 (x) S 2 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 7 1 1"],
  [24, "24 (x) S 2 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 7 1 1 "],
  [25, "25 (x) S 2 1 1 1 1 1 1 1 1 1 1 1 2 3 4 5 6 7 8 9 10 11"],
];

// prettier-ignore
const PS_TIMES = [
  "0:00", "00:01.50", "12:34:56", "1-02:03:04", "1-2:03:04", "100-00:00:00.99", "5:07", "5:7",
  "05:07.5", "1:2:3:4", "", "abc", "-1:00", "1:00.", "1:.5", "00:00:00", "1.5:00",
  "9999999999-00:00:00",
];

const PS_LINES = [
  "  PID  PPID      TIME COMM\n    1     0   0:12.34 /sbin/launchd\n  100     1   1-02:03:04 /bin/zsh\n  101   100   00:01.50 /Applications/Visual Studio Code.app/Contents/MacOS/Electron\n",
  "1 0 0:01 init\r\n2 1 0:02 a b  \r\n",
  "  7   1  00:00:01   \n",
  "7 1 00:00:01 \u2028\n",
  "7 1 00:00:01 a\u2028b\n8 1 00:00:02 c\u2028\n",
  "7\t1\t0:01\tbash\n",
  "x 1 0:01 a\n7 y 0:01 a\n7 1 zz a\n7 1 0:01\n7 1\n7\n\n",
  "\u00a07 1 0:01 nbsp\n\ufeff8 1 0:01 bom\n",
  "9 1 0:01 -zsh\n10 9 0:00.50 npm\n",
];

function entry(
  pid: number,
  ppid: number,
  comm: string,
  cpuMs: number,
  startKey = String(pid),
): ProcessEntry {
  return { pid, ppid, comm, cpuMs, startKey };
}

const TABLES: Array<[string, ProcessEntry[], number]> = [
  [
    "mcp-and-tools",
    [
      entry(1, 0, "systemd", 0),
      entry(10, 1, "bash", 10),
      entry(11, 10, "claude", 500),
      entry(12, 11, "bash", 100),
      entry(13, 12, "npm", 200),
      entry(14, 13, "node", 300),
      entry(15, 11, "mcp-server", 5),
      entry(16, 11, "-zsh", 1),
      entry(17, 16, "sleep", 2),
      entry(18, 11, "/usr/bin/fish", 3),
    ],
    10,
  ],
  [
    "cycle",
    [
      entry(10, 1, "bash", 0),
      entry(11, 10, "c", 0),
      entry(12, 11, "sh", 0),
      entry(11, 12, "x", 0),
    ],
    10,
  ],
  ["none", [entry(10, 1, "bash", 0), entry(11, 10, "claude", 0)], 10],
  ["empty", [], 10],
  [
    "shell-pid-reused",
    [
      entry(10, 10, "bash", 0),
      entry(11, 10, "claude", 0),
      entry(12, 11, "sh", 0),
    ],
    10,
  ],
];

interface WalkerScenario {
  name: string;
  self: number;
  retryMs: number;
  threads: Record<string, string[]>;
  children: Record<string, string>;
  stats: Record<string, string>;
  failures: Record<string, string>;
  steps: Array<{ now: number; shellPid: number }>;
}

function walkerScenarios(): WalkerScenario[] {
  const stat = (pid: number, ppid: number, comm: string, cpu = 0): string =>
    `${pid} (${comm}) S ${ppid} 1 1 0 -1 0 0 0 0 0 ${cpu} 0 0 0 20 0 1 0 ${pid * 7} 0 0`;
  const tree = {
    threads: { "10": ["10"], "11": ["11", "12"], "12": ["12"], "13": ["13"] },
    children: {
      "10/10": "11 ",
      "11/11": "12 ",
      "11/12": "15 ",
      "12/12": "13 ",
      "13/13": "",
      "15/15": "",
    },
    stats: {
      "11": stat(11, 10, "claude", 50),
      "12": stat(12, 11, "bash", 100),
      "13": stat(13, 12, "node", 200),
      "15": stat(15, 11, "mcp", 3),
    },
  };
  return [
    {
      name: "children-lists",
      self: 100,
      retryMs: 60_000,
      ...tree,
      children: { ...tree.children, "100/100": "" },
      failures: {},
      steps: [
        { now: 0, shellPid: 10 },
        { now: 1, shellPid: 10 },
      ],
    },
    {
      name: "no-children-file",
      self: 100,
      retryMs: 60_000,
      ...tree,
      failures: { "children/100/100": "ENOENT" },
      steps: [
        { now: 0, shellPid: 10 },
        { now: 100_000, shellPid: 10 },
      ],
    },
    {
      name: "transient-failure-retries-later",
      self: 100,
      retryMs: 1_000,
      ...tree,
      children: { ...tree.children, "100/100": "" },
      failures: { "stat/11": "EMFILE" },
      steps: [
        { now: 0, shellPid: 10 },
        { now: 500, shellPid: 10 },
        { now: 1_500, shellPid: 10 },
      ],
    },
    {
      name: "vanishing-processes",
      self: 100,
      retryMs: 1_000,
      ...tree,
      children: { ...tree.children, "100/100": "" },
      failures: {
        "threads/11": "ENOENT",
        "children/12/12": "ESRCH",
        "stat/15": "ENOENT",
      },
      steps: [{ now: 0, shellPid: 10 }],
    },
    {
      name: "unreadable-probe",
      self: 100,
      retryMs: 1_000,
      ...tree,
      failures: { "children/100/100": "EACCES" },
      steps: [
        { now: 0, shellPid: 10 },
        { now: 2_000, shellPid: 10 },
      ],
    },
  ];
}

function scenarioIo(scenario: WalkerScenario): ProcIo {
  const fail = (key: string): void => {
    const code = scenario.failures[key];
    if (code !== undefined) throw Object.assign(new Error(code), { code });
  };
  return {
    threads: async (pid) => {
      fail(`threads/${pid}`);
      const list = scenario.threads[String(pid)];
      if (list === undefined)
        throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      return list;
    },
    children: async (pid, thread) => {
      fail(`children/${pid}/${thread}`);
      const list = scenario.children[`${pid}/${thread}`];
      if (list === undefined)
        throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      return list;
    },
    stat: async (pid) => {
      fail(`stat/${pid}`);
      const content = scenario.stats[String(pid)];
      if (content === undefined)
        throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
      return content;
    },
  };
}

const entryJson = (value: ProcessEntry): Json => ({ ...value });

async function processCases(): Promise<Json> {
  const walkers: Json[] = [];
  for (const scenario of walkerScenarios()) {
    let now = 0;
    const walker = new ToolProcessWalker(
      scenarioIo(scenario),
      () => now,
      scenario.retryMs,
      scenario.self,
    );
    const results: Json[] = [];
    for (const step of scenario.steps) {
      now = step.now;
      const found = await walker.read(step.shellPid);
      results.push(found === undefined ? null : found.map(entryJson));
    }
    walkers.push({
      ...(scenario as unknown as { [key: string]: Json }),
      results,
    });
  }
  const trackerRuns: Array<[string, Array<[string, ProcessEntry[], number]>]> =
    [
      [
        "baseline-then-growth",
        [
          ["a", [entry(1, 0, "x", 100, "s1")], 1000],
          ["a", [entry(1, 0, "x", 250, "s1")], 2000],
          ["a", [entry(1, 0, "x", 450, "s1")], 3000],
          ["a", [entry(1, 0, "x", 450, "s1")], 4000],
        ],
      ],
      [
        "new-process-counts-whole",
        [
          ["a", [entry(1, 0, "x", 0, "s1")], 1000],
          ["a", [entry(1, 0, "x", 0, "s1"), entry(2, 1, "y", 500, "s2")], 2000],
          ["a", [entry(2, 1, "y", 500, "s2")], 3000],
        ],
      ],
      [
        "reused-pid",
        [
          ["a", [entry(1, 0, "x", 900, "s1")], 1000],
          ["a", [entry(1, 0, "x", 100, "s9")], 2000],
          ["a", [entry(1, 0, "x", 100, "s9")], 3000],
        ],
      ],
      [
        "two-agents",
        [
          ["a", [entry(1, 0, "x", 0, "s")], 1],
          ["b", [entry(1, 0, "x", 0, "s")], 2],
          ["a", [entry(1, 0, "x", 300, "s")], 3],
          ["b", [entry(1, 0, "x", 100, "s")], 4],
        ],
      ],
      [
        "below-threshold",
        [
          ["a", [entry(1, 0, "x", 0, "s")], 1],
          ["a", [entry(1, 0, "x", 199, "s")], 2],
          ["a", [entry(1, 0, "x", 398, "s")], 3],
        ],
      ],
      [
        "empty-samples",
        [
          ["a", [], 1],
          ["a", [entry(1, 0, "x", 500, "s")], 2],
          ["a", [], 3],
        ],
      ],
    ];
  const trackers = trackerRuns.map(([name, steps]) => {
    const tracker = new ProcessActivityTracker();
    const states: Json[] = [];
    for (const [agent, processes, at] of steps) {
      tracker.record(agent, { processes }, at);
      states.push(
        steps
          .map(([id]) => id)
          .filter((id, index, all) => all.indexOf(id) === index)
          .map((id) => [id, orNull(tracker.lastChildActivity(id))]),
      );
    }
    tracker.forget("a");
    states.push([
      ["a", orNull(tracker.lastChildActivity("a"))],
      ["activity", Array.from(tracker.activity().entries())],
    ]);
    return {
      name,
      steps: steps.map(([agent, processes, at]) => ({
        agent,
        processes: processes.map(entryJson),
        at,
      })),
      states,
    };
  });
  const probeCases: Json[] = [];
  const infoFixture = text("herdr-process-info.json");
  const probeInputs: Array<[string, HerdrResult]> = [
    ["fixture", { code: 0, stdout: infoFixture, stderr: "" }],
    [
      "no-shell-pid",
      {
        code: 0,
        stdout: '{"result":{"process_info":{"pane_id":"w1:p1"}}}',
        stderr: "",
      },
    ],
    [
      "zero-shell-pid",
      {
        code: 0,
        stdout: '{"result":{"process_info":{"shell_pid":0}}}',
        stderr: "",
      },
    ],
    [
      "float-shell-pid",
      {
        code: 0,
        stdout: '{"result":{"process_info":{"shell_pid":1.5}}}',
        stderr: "",
      },
    ],
    [
      "string-shell-pid",
      {
        code: 0,
        stdout: '{"result":{"process_info":{"shell_pid":"7"}}}',
        stderr: "",
      },
    ],
    [
      "failure",
      {
        code: 1,
        stdout: "",
        stderr: '{"error":{"code":"pane_not_found","message":"no pane"}}',
      },
    ],
  ];
  const probeTable: ProcessEntry[] = [
    entry(31358, 1, "bash", 0),
    entry(31400, 31358, "claude", 1000),
    entry(31401, 31400, "bash", 400),
    entry(31402, 31401, "cargo", 900),
  ];
  for (const [name, outcome] of probeInputs) {
    const calls: string[][] = [];
    const probe = new HerdrProcessProbe(
      async (args) => {
        calls.push([...args]);
        return outcome;
      },
      async () => probeTable,
    );
    probeCases.push({
      name,
      outcome: outcome as unknown as Json,
      table: probeTable.map(entryJson),
      calls,
      result: await probe.sample("w1:p1").then(
        (sample) => ({ ok: sample.processes.map(entryJson) }),
        (error: unknown) => ({
          error: error instanceof Error ? error.message : String(error),
        }),
      ),
    });
  }
  return {
    procStats: PROC_STATS.map(([pid, content]) => ({
      pid,
      content,
      ticks: 100,
      result: orNull(parseProcStat(pid, content)) as Json,
    })),
    procStatTicks: [50, 250, 1000].map((ticks) => ({
      ticks,
      content: PROC_STATS[0]![1],
      result: orNull(parseProcStat(10, PROC_STATS[0]![1], ticks)) as Json,
    })),
    psTimes: PS_TIMES.map((value) => [value, orNull(parsePsTime(value))]),
    psTables: [...PS_LINES, text("ps-macos.txt")].map((value) => ({
      text: value,
      entries: parsePsTable(value).map(entryJson),
    })),
    tables: TABLES.map(([name, table, shellPid]) => ({
      name,
      shellPid,
      table: table.map(entryJson),
      counted: toolProcesses(table, shellPid).map(entryJson),
    })),
    walkers,
    trackers,
    probes: probeCases,
  };
}

// --------------------------------------------------------------------------------------------------------- adapter

interface Context {
  readonly slug: string | undefined;
  readonly fake: FakeHerdr;
  readonly vars: Record<string, string>;
  /** Answers to give instead of the fake's, for the next call whose argv matches. */
  readonly overrides: Array<{
    match: (args: readonly string[]) => boolean;
    response: HerdrResult;
  }>;
}

type Step =
  | { readonly fake: (context: Context) => void }
  | {
      readonly op: string;
      readonly args: (context: Context) => Record<string, Json>;
      readonly save?: (result: Json, context: Context) => void;
    };

interface Scenario {
  readonly name: string;
  readonly slug?: string;
  readonly steps: readonly Step[];
}

class StaleActionError extends Error {
  override readonly name = "StaleActionError";
}

function idle(...typed: string[]): string {
  return idleScreen(...typed);
}

function dialogScreen(checkout: string, selected: "no" | "yes" = "no"): string {
  const content = text("claude-trust-dialog.txt").replace(
    "/home/user/.herdr/worktrees/probe-repo-FQ8H/probe-two",
    checkout,
  );
  return selected === "no"
    ? content
    : content
        .replace(`❯ ${TRUST_NO}`, `  ${TRUST_NO}`)
        .replace(`  ${TRUST_YES}`, `❯ ${TRUST_YES}`);
}

function dialogKeys(
  pane: FakePane,
  checkout: string,
): (target: FakePane, key: string) => void {
  let selected: "no" | "yes" = pane.screen.includes(`❯ ${TRUST_YES}`)
    ? "yes"
    : "no";
  return (target, key) => {
    if (target !== pane) return;
    if (key === "down") selected = "yes";
    if (key === "up") selected = "no";
    if (key === "enter") {
      pane.status = "idle";
      pane.screen =
        selected === "yes" ? idle() : dialogScreen(checkout, selected);
      if (selected === "yes") return;
    } else pane.screen = dialogScreen(checkout, selected);
  };
}

const promptFixture = (name: string): string =>
  text(path.join("prompts", name));

/** A Claude permission prompt that reacts to keys like the real one. */
class PromptScript {
  selected = 0;
  field: string | undefined;
  readonly base = parseHostPrompt(
    "claude",
    promptFixture("claude-bash-permission.ansi"),
  )!;
  readonly texts = this.base.options.map((option) => option.text);
  tabDoesNothing = false;
  arrowsStuck = false;
  constructor(readonly pane: FakePane) {}

  render(): string {
    const rows = this.texts.map((value, index) => {
      const shown =
        index === this.selected && this.field !== undefined
          ? this.field
          : value;
      return ` ${index === this.selected ? "❯" : " "} ${index + 1}. ${shown}`;
    });
    return [
      "earlier output",
      "─".repeat(100),
      ...this.base.text.split("\n"),
      ...rows,
      "",
      this.field === undefined
        ? " Esc to cancel · Tab to amend"
        : " Esc to cancel",
    ].join("\r\n");
  }

  attach(fake: FakeHerdr): void {
    this.pane.screen = this.render();
    fake.onKey = (pane, key) => {
      if (pane !== this.pane) return;
      if (key === "down" && !this.arrowsStuck)
        this.selected = Math.min(this.selected + 1, this.texts.length - 1);
      if (key === "up") this.selected = Math.max(this.selected - 1, 0);
      if (key === "tab" && !this.tabDoesNothing) {
        const wording = TEXT_FIELD_WORDING[this.texts[this.selected]!];
        if (wording !== undefined) this.field = wording;
      }
      if (key === "enter" || key === "esc") {
        pane.status = "idle";
        pane.screen = idle();
        return;
      }
      pane.screen = this.render();
    };
    fake.onText = (pane, value) => {
      if (pane !== this.pane || this.field === undefined) return;
      this.field = `${this.texts[this.selected]}, ${value}`;
      pane.screen = this.render();
    };
  }
}

/** The state Herdr holds for an agent, under the name Herdr knows it by. */
const setAgent = (
  context: Context,
  name: string,
  state: { paneId: string; statuses: string[]; kind?: string },
): void => {
  context.fake.agentStates.set(
    context.slug === undefined ? name : `${context.slug}-${name}`,
    state,
  );
};

const op = (
  name: string,
  args:
    Record<string, Json> | ((context: Context) => Record<string, Json>) = {},
  save?: (result: Json, context: Context) => void,
): Step => ({
  op: name,
  args: typeof args === "function" ? args : () => args,
  ...(save === undefined ? {} : { save }),
});
const fake = (action: (context: Context) => void): Step => ({ fake: action });
const pane = (context: Context, name = "pane"): FakePane =>
  context.fake.panes.get(context.vars[name]!)!;
const saveAs =
  (name: string, field: string) =>
  (result: Json, context: Context): void => {
    const value = ((result as { ok?: { [key: string]: Json } }).ok ?? {})[
      field
    ];
    if (typeof value === "string") context.vars[name] = value;
  };

const wt = (name = "pane", branch = "b1"): Step =>
  op(
    "createWorktree",
    { workspaceId: "w9", branch, label: "dev" },
    saveAs(name, "paneId"),
  );
const start = (extra: Record<string, Json> = {}): Step =>
  op("startAgent", (context) => ({
    name: "dev",
    kind: "claude",
    paneId: context.vars.pane!,
    args: [],
    environment: CLEAN_ENV,
    ...extra,
  }));
const clear = (extra: Record<string, Json> = {}): Step =>
  op(
    "clearAfterDeferral",
    onPane({ deferredForMs: 100, maxDeferralMs: 100, ...extra }),
  );
const adopt = (extra: Record<string, Json> = {}): Step =>
  op(
    "adoptPane",
    onPane({
      role: "worker",
      agent: "dev",
      workspaceId: null,
      worktreePath: null,
      ...extra,
    }),
  );
const place = (extra: Record<string, Json> = {}): Step =>
  op("placePane", (context) => ({
    paneId: context.vars.pane!,
    tabId: "w1:t1",
    targetPaneId: "w1:p1",
    direction: "right",
    keep: 0.5,
    worktreePath: context.vars.checkout ?? pane(context).checkout,
    ...extra,
  }));
const screen = (content: string): Step =>
  fake((context) => {
    pane(context).screen = content;
  });
const agentIs = (...statuses: string[]): Step =>
  fake((context) => {
    setAgent(context, "dev", { paneId: context.vars.pane!, statuses });
  });

/** A fresh worktree pane with a started claude agent `dev`, as test/herdr-adapter-harness.ts `startedWorker`. */
const startWorker = (
  screen: string | (() => string) = () => idle(),
  statuses: string[] = ["idle"],
  kind = "claude",
  agent = "dev",
): Step[] => [
  op(
    "createWorktree",
    { workspaceId: "w9", branch: "cap/task/dev-g1", label: "dev" },
    saveAs("pane", "paneId"),
  ),
  op("startAgent", (context) => ({
    name: agent,
    kind,
    paneId: context.vars.pane!,
    args: ["--model", "x"],
    environment: CLEAN_ENV,
  })),
  fake((context) => {
    const target = pane(context);
    target.screen = typeof screen === "string" ? screen : screen();
    setAgent(context, agent, {
      paneId: target.paneId,
      statuses: [...statuses],
      kind,
    });
  }),
];

const startPm = (screen: string = idle(), name = "pane"): Step[] => [
  op(
    "createWorkspace",
    { cwd: "/tmp", label: "proj", role: "PM" },
    saveAs(name, "paneId"),
  ),
  op("startAgent", (context) => ({
    name: "pm-1",
    kind: "claude",
    paneId: context.vars[name]!,
    args: [],
    environment: CLEAN_ENV,
  })),
  fake((context) => {
    pane(context, name).screen = screen;
    setAgent(context, "pm-1", {
      paneId: context.vars[name]!,
      statuses: ["idle"],
    });
  }),
];

const onPane =
  (extra: Record<string, Json> = {}) =>
  (context: Context): Record<string, Json> => ({
    paneId: context.vars.pane!,
    ...extra,
  });

const send = (value: string, extra: Record<string, Json> = {}): Step =>
  op("guardedSend", (context) => ({
    paneId: context.vars.pane!,
    text: value,
    ...extra,
  }));

const blockedWorker = (selected: "no" | "yes" = "no"): Step[] => [
  ...startWorker(),
  fake((context) => {
    const target = pane(context);
    const checkout = target.checkout;
    target.screen = dialogScreen(checkout, selected);
    target.status = "blocked";
    setAgent(context, "dev", {
      paneId: target.paneId,
      statuses: ["blocked"],
    });
    context.fake.onKey = dialogKeys(target, checkout);
  }),
];

const promptWorker = (kind = "claude"): Step[] => [
  ...startWorker(() => idle(), ["blocked"], kind),
  fake((context) => {
    const target = pane(context);
    target.status = "blocked";
    const script = new PromptScript(target);
    script.attach(context.fake);
    (context as unknown as { script: PromptScript }).script = script;
  }),
];

const captureAndSave = op("capturePrompt", onPane(), (result, context) => {
  const outcome = (result as { ok?: { prompt?: { promptSha?: string } } }).ok;
  const value = outcome?.prompt?.promptSha;
  if (typeof value === "string") context.vars.sha = value;
});

const answerWith = (
  answer: Record<string, Json>,
  sha: string | ((context: Context) => string) = (context) => context.vars.sha!,
): Step =>
  op("answerPrompt", (context) => ({
    paneId: context.vars.pane!,
    promptSha: typeof sha === "string" ? sha : sha(context),
    answer,
  }));

function scenarios(): Scenario[] {
  const list: Scenario[] = [];
  const add = (name: string, steps: Step[], slug?: string): void => {
    list.push({ name, steps, ...(slug === undefined ? {} : { slug }) });
  };

  add("launch-and-reads", [
    op("version"),
    ...startWorker(),
    op("paneEntry", onPane()),
    op("paneForAgent", { agentId: "dev" }),
    op("agentObservation", { agentId: "dev" }),
    op("agentState", { name: "dev" }),
    op("paneState", onPane()),
    op("readInput", onPane()),
    op("readScreen", onPane({ ansi: true, lines: 20 })),
    op("readScreen", onPane()),
    op("runInPane", onPane({ command: "ls" })),
    op("forgetPane", onPane()),
    op("paneEntry", onPane()),
  ]);
  add(
    "launch-with-slug",
    [
      ...startWorker(),
      op("agentState", { name: "dev" }),
      op("agentObservation", { agentId: "dev" }),
      send("hello there"),
      op("agentState", { name: "bad name" }),
      op("agentObservation", { agentId: "nobody" }),
    ],
    "acme",
  );
  add("start-failures", [
    op("startAgent", { name: "dev", kind: "vim", paneId: "w1:p1", args: [] }),
    op("startAgent", {
      name: "bad name",
      kind: "claude",
      paneId: "w1:p1",
      args: [],
    }),
    op("startAgent", {
      name: "dev",
      kind: "claude",
      paneId: "w1:p1",
      args: [],
    }),
    wt(),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [""],
      environment: CLEAN_ENV,
    })),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: ["a\nb"],
      environment: CLEAN_ENV,
    })),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [],
    })),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [],
      environment: { HOME: "relative", PATH: "/p", TERM: "t" },
    })),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [],
      environment: { HOME: "/h", PATH: "/p" },
    })),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [],
      environment: { HOME: "/h", PATH: "/p", TERM: "t", "bad-key": "1" },
    })),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [],
      environment: { HOME: "/h", PATH: "/p", TERM: "t", GOOD: "a\u2028b" },
    })),
    fake((context) => {
      context.fake.startError = { code: "boom", message: "it broke" };
    }),
    start(),
    op("paneEntry", onPane()),
    send("x"),
    op("startAgent", (context) => ({
      name: "dev2",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [],
      environment: CLEAN_ENV,
    })),
  ]);
  add("start-blocked-at-startup", [
    wt(),
    fake((context) => {
      context.fake.startError = {
        code: "agent_not_ready",
        message: "not ready",
      };
    }),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "codex",
      paneId: context.vars.pane!,
      args: ["--x"],
      environment: CLEAN_ENV,
      timeoutMs: 1000,
    })),
    op("paneEntry", onPane()),
    start(),
  ]);
  add("start-prepared-pane", [
    wt(),
    op("prepareShell", (context) => ({
      paneId: context.vars.pane!,
      environment: { ...CLEAN_ENV, EXTRA: "it's" },
    })),
    op("paneEntry", onPane()),
    op("prepareShell", (context) => ({
      paneId: context.vars.pane!,
      environment: CLEAN_ENV,
    })),
    screen("something typed\r\n❯ text"),
    op("startAgent", (context) => ({
      name: "dev",
      kind: "claude",
      paneId: context.vars.pane!,
      args: [],
    })),
    op("paneEntry", onPane()),
  ]);
  add("prepare-timeout", [
    wt(),
    fake((context) => {
      context.fake.onRun = undefined;
    }),
    op("prepareShell", (context) => ({
      paneId: context.vars.pane!,
      environment: CLEAN_ENV,
      timeoutMs: 500,
    })),
    op("paneEntry", onPane()),
    send("x"),
  ]);
  add("fresh-prompt-wait", [
    wt(),
    screen("no prompt here"),
    op("runInPane", onPane({ command: "ls" })),
    op("prepareShell", (context) => ({
      paneId: context.vars.pane!,
      environment: CLEAN_ENV,
    })),
    screen(SHELL_READY),
    op("runInPane", onPane({ command: "" })),
    op("runInPane", onPane({ command: "a\nb" })),
    op("runInPane", onPane({ command: "x".repeat(2001) })),
    op("runInPane", onPane({ command: "echo ok" })),
    op("runInPane", onPane({ command: "echo again" })),
    op("runInPane", { paneId: "w77:p1", command: "ls" }),
  ]);

  // ---- guarded send
  for (const [status, label] of [
    ["idle", "idle"],
    ["done", "done"],
    ["working", "working"],
    ["blocked", "blocked"],
    ["unknown", "unknown"],
    ["brand-new", "other"],
  ] as const)
    add(`guarded-send-status-${label}`, [
      ...startWorker(() => idle(), [status]),
      send("do the thing"),
    ]);
  for (const [label, screen] of [
    ["typed", idle("half typed")],
    ["multi-line", idle("a", "b")],
    ["unreadable", "no input box here"],
    ["permission-prompt", promptFixture("claude-bash-permission.ansi")],
    ["dialog", promptFixture("synthetic-dialog-teach-auto-mode.ansi")],
  ] as const)
    add(`guarded-send-input-${label}`, [
      ...startWorker(() => screen),
      send("do the thing"),
    ]);
  add("guarded-send-changes-state", [
    ...startWorker(() => idle(), ["idle", "idle", "working"]),
    send("do the thing"),
    send("do the thing"),
  ]);
  add("guarded-send-validation", [
    ...startWorker(),
    send(""),
    send("   "),
    send("/command"),
    send("\ttab"),
    send("  @mention"),
    send("bell\u0007"),
    send("zero\u200bwidth"),
    send("x".repeat(16 * 1024 + 1)),
    send("é".repeat(8 * 1024 + 1)),
    send("-dash is fine"),
    send("multi\nline\n\ttab\u200djoiner"),
    send("é".repeat(8 * 1024)),
  ]);
  add("guarded-send-hooks", [
    ...startWorker(),
    send("first", { beforeSend: "throw" }),
    fake((context) => {
      context.overrides.push({
        match: (args) => args[0] === "agent" && args[1] === "prompt",
        response: {
          code: 1,
          stdout: "",
          stderr: '{"error":{"code":"refused","message":"no thanks"}}',
        },
      });
    }),
    send("second"),
    send("third"),
  ]);
  add("guarded-send-pane-checks", [
    op("guardedSend", { paneId: "w5:p5", text: "x" }),
    ...startWorker(),
    fake((context) => {
      setAgent(context, "dev", {
        paneId: "w9:p9",
        statuses: ["idle"],
      });
    }),
    send("x"),
    op("agentObservation", { agentId: "dev" }),
    ...startPm(),
    op("guardedSend", { paneId: "w2:p1", text: "x" }),
  ]);

  // ---- wake PM
  add(
    "wake-pm",
    [
      ...startPm(),
      op("wakePm", onPane({ text: "Run cstan inbox: a teammate wrote." })),
      op("wakePm", onPane({ text: "Run cstan inbox", beforeSend: "throw" })),
      screen(idle("typing")),
      op("wakePm", onPane({ text: "Run cstan inbox" })),
      screen("no box"),
      op("wakePm", onPane({ text: "Run cstan inbox" })),
      fake((context) => {
        pane(context).screen = idle();
        setAgent(context, "pm-1", {
          paneId: context.vars.pane!,
          statuses: ["working"],
        });
      }),
      op("wakePm", onPane({ text: "Run cstan inbox" })),
      fake((context) => {
        setAgent(context, "pm-1", {
          paneId: context.vars.pane!,
          statuses: ["blocked"],
        });
      }),
      op("wakePm", onPane({ text: "Run cstan inbox" })),
      fake((context) => {
        setAgent(context, "pm-1", {
          paneId: context.vars.pane!,
          statuses: ["idle", "idle", "working"],
        });
      }),
      op("wakePm", onPane({ text: "Run cstan inbox" })),
      op("wakePm", onPane({ text: "line\nbreak" })),
      op("wakePm", onPane({ text: "" })),
      op("wakePm", onPane({ text: "/slash" })),
      op("wakePm", { paneId: "bad", text: "x" }),
      op("wakePm", { paneId: "w44:p1", text: "x" }),
    ],
    "acme",
  );
  add("wake-pm-worker-pane", [
    ...startWorker(),
    op("wakePm", onPane({ text: "x" })),
    op(
      "createWorkspace",
      { cwd: "/tmp", label: "proj", role: "PM" },
      saveAs("pm", "paneId"),
    ),
    op("wakePm", (context) => ({ paneId: context.vars.pm!, text: "x" })),
  ]);

  // ---- clear after deferral
  add("clear-after-deferral", [
    ...startWorker(() => idle("some typed text")),
    fake((context) => {
      context.fake.onKey = (target, key) => {
        if (key === "ctrl+u") target.screen = idle();
      };
    }),
    op("clearAfterDeferral", onPane({ deferredForMs: 10, maxDeferralMs: 100 })),
    clear(),
    clear(),
  ]);
  add("clear-validation", [
    ...startWorker(() => idle("typed")),
    op("clearAfterDeferral", onPane({ deferredForMs: -1, maxDeferralMs: 100 })),
    op("clearAfterDeferral", onPane({ deferredForMs: 100, maxDeferralMs: 0 })),
    op(
      "clearAfterDeferral",
      onPane({ deferredForMs: 1e999, maxDeferralMs: 100 }),
    ),
    op("clearAfterDeferral", onPane({ deferredForMs: 5, maxDeferralMs: 100 })),
    op("clearAfterDeferral", {
      paneId: "w50:p1",
      deferredForMs: 100,
      maxDeferralMs: 100,
    }),
  ]);
  for (const [label, status] of [
    ["working", "working"],
    ["blocked", "blocked"],
  ] as const)
    add(`clear-refuses-${label}`, [
      ...startWorker(() => idle("typed"), [status]),
      clear(),
    ]);
  add("clear-unreadable", [
    ...startWorker(() => promptFixture("claude-bash-permission.ansi")),
    clear(),
    fake((context) => {
      pane(context).screen = idle("typed");
      context.fake.onKey = (target) => {
        target.screen = "gone";
      };
    }),
    clear(),
  ]);
  add("clear-multi-round", [
    ...startWorker(() => idle("one", "two", "three")),
    fake((context) => {
      let lines = ["one", "two", "three"];
      context.fake.onKey = (target, key) => {
        if (key !== "ctrl+u") return;
        lines = lines.slice(1);
        target.screen = idle(...lines);
      };
    }),
    op(
      "clearAfterDeferral",
      onPane({ deferredForMs: 100, maxDeferralMs: 100, discard: "record" }),
    ),
  ]);
  add("clear-never-clears", [...startWorker(() => idle("stuck")), clear()]);
  add("clear-discard-throws", [
    ...startWorker(() => idle("typed")),
    op(
      "clearAfterDeferral",
      onPane({ deferredForMs: 100, maxDeferralMs: 100, discard: "throw" }),
    ),
  ]);
  add("clear-agent-stops-idle", [
    ...startWorker(() => idle("typed"), ["idle", "idle", "working"]),
    fake((context) => {
      context.fake.onKey = (target, key) => {
        if (key === "ctrl+u") target.screen = idle("again");
      };
    }),
    clear(),
  ]);
  add("clear-new-text-between-rounds", [
    ...startWorker(() => idle("first")),
    fake((context) => {
      let round = 0;
      context.fake.onKey = (target, key) => {
        if (key !== "ctrl+u") return;
        round += 1;
        target.screen = round === 1 ? idle("second") : idle();
      };
    }),
    clear(),
  ]);

  // ---- trust dialog
  add("trust-dialog-down-then-enter", [
    ...blockedWorker("no"),
    op("answerTrustDialog", onPane()),
    op("answerTrustDialog", onPane()),
  ]);
  add("trust-dialog-already-yes", [
    ...blockedWorker("yes"),
    op("answerTrustDialog", onPane()),
  ]);
  add("trust-dialog-other-path", [
    ...startWorker(),
    fake((context) => {
      const target = pane(context);
      target.screen = dialogScreen("/some/other/place");
      target.status = "blocked";
      setAgent(context, "dev", {
        paneId: target.paneId,
        statuses: ["blocked"],
      });
    }),
    op("answerTrustDialog", onPane()),
  ]);
  for (const [label, content] of [
    ["unknown-options", dialogScreen("{checkout}").replace(TRUST_YES, "Sure")],
    ["not-last", `${dialogScreen("{checkout}")}\n\nextra output after`],
    ["no-dialog", idle()],
  ] as const)
    add(`trust-dialog-${label}`, [
      ...startWorker(),
      fake((context) => {
        const target = pane(context);
        target.screen = content.replaceAll("{checkout}", target.checkout);
        target.status = "blocked";
        setAgent(context, "dev", {
          paneId: target.paneId,
          statuses: ["blocked"],
        });
      }),
      op("answerTrustDialog", onPane()),
    ]);
  add("trust-dialog-wrapped-path", [
    ...startWorker(),
    fake((context) => {
      const target = pane(context);
      target.screen = [
        "Accessing workspace:",
        "",
        " /very/long",
        " wrapped/path",
        "",
        " Enter to confirm",
      ].join("\n");
      target.status = "blocked";
      setAgent(context, "dev", {
        paneId: target.paneId,
        statuses: ["blocked"],
      });
    }),
    op("answerTrustDialog", onPane()),
  ]);
  add("trust-dialog-needs-blocked", [
    ...startWorker(),
    op("answerTrustDialog", onPane()),
    ...startPm(),
    op("answerTrustDialog", { paneId: "w2:p1" }),
    op("answerTrustDialog", { paneId: "w40:p1" }),
  ]);
  add("trust-dialog-never-reaches-selection", [
    ...blockedWorker("no"),
    fake((context) => {
      context.fake.onKey = undefined;
    }),
    op("answerTrustDialog", onPane()),
  ]);
  add("trust-dialog-stays-open", [
    ...blockedWorker("yes"),
    fake((context) => {
      const target = pane(context);
      context.fake.onKey = () => {
        target.screen = dialogScreen(target.checkout, "yes");
      };
    }),
    op("answerTrustDialog", onPane({ timeoutMs: 500 })),
  ]);
  add("trust-dialog-codex-and-omp", [
    ...startWorker(() => idle(), ["blocked"], "omp", "ompdev"),
    op("answerTrustDialog", onPane()),
    op(
      "createWorktree",
      { workspaceId: "w9", branch: "b2", label: "cx" },
      saveAs("cx", "paneId"),
    ),
    op("startAgent", (context) => ({
      name: "cx",
      kind: "codex",
      paneId: context.vars.cx!,
      args: [],
      environment: CLEAN_ENV,
    })),
    fake((context) => {
      const target = context.fake.panes.get(context.vars.cx!)!;
      target.screen = [
        "Folder access",
        target.checkout,
        "Do you trust this folder?",
        "› 1. Trust and continue",
        "  2. Quit",
        "",
        "enter continue · esc quit",
      ].join("\n");
      setAgent(context, "cx", {
        paneId: target.paneId,
        statuses: ["blocked"],
        kind: "codex",
      });
      context.fake.onKey = (_target, key) => {
        if (key === "enter") target.screen = "$ ";
      };
    }),
    op("answerTrustDialog", (context) => ({ paneId: context.vars.cx! })),
  ]);

  // ---- prompt relay
  add("prompt-capture", [
    ...promptWorker(),
    captureAndSave,
    fake((context) => {
      pane(context).status = "idle";
    }),
    op("capturePrompt", onPane()),
    fake((context) => {
      pane(context).status = "blocked";
      pane(context).screen = idle();
    }),
    op("capturePrompt", onPane()),
  ]);
  add("prompt-capture-other-hosts", [
    ...startWorker(() => idle(), ["blocked"], "codex"),
    op("capturePrompt", onPane()),
    ...startPm(),
    op("capturePrompt", { paneId: "w2:p1" }),
    op("capturePrompt", { paneId: "w31:p1" }),
  ]);
  add("prompt-answer-option", [
    ...promptWorker(),
    captureAndSave,
    answerWith({ kind: "option", number: 4 }),
  ]);
  add("prompt-answer-already-selected", [
    ...promptWorker(),
    captureAndSave,
    answerWith({ kind: "option", number: 1 }),
  ]);
  add("prompt-answer-widening", [
    ...promptWorker(),
    captureAndSave,
    answerWith({ kind: "option", number: 3 }),
  ]);
  add("prompt-answer-up", [
    ...promptWorker(),
    captureAndSave,
    fake((context) => {
      const script = (context as unknown as { script: PromptScript }).script;
      script.selected = 3;
      script.pane.screen = script.render();
    }),
    captureAndSave,
    answerWith({ kind: "option", number: 2 }),
  ]);
  add("prompt-answer-esc", [
    ...promptWorker(),
    captureAndSave,
    answerWith({ kind: "esc" }),
  ]);
  add("prompt-answer-text", [
    ...promptWorker(),
    captureAndSave,
    answerWith({ kind: "text", number: 4, text: "do not touch files" }),
  ]);
  add("prompt-answer-text-refused", [
    ...promptWorker(),
    captureAndSave,
    answerWith({ kind: "text", number: 4, text: "/slash" }),
    answerWith({ kind: "text", number: 1, text: "fine" }),
    answerWith({ kind: "text", number: 2, text: "fine" }),
    answerWith({ kind: "text", number: 9, text: "fine" }),
    answerWith({ kind: "option", number: 9 }),
  ]);
  add("prompt-answer-stale-hash", [
    ...promptWorker(),
    captureAndSave,
    answerWith({ kind: "option", number: 2 }, "0".repeat(64)),
  ]);
  add("prompt-answer-not-blocked", [
    ...promptWorker(),
    captureAndSave,
    agentIs("working"),
    answerWith({ kind: "option", number: 2 }),
    fake((context) => {
      setAgent(context, "dev", {
        paneId: "w9:p9",
        statuses: ["blocked"],
      });
    }),
    answerWith({ kind: "option", number: 2 }),
  ]);
  add("prompt-answer-selection-never-reached", [
    ...promptWorker(),
    captureAndSave,
    fake((context) => {
      (context as unknown as { script: PromptScript }).script.arrowsStuck =
        true;
    }),
    answerWith({ kind: "option", number: 3 }),
  ]);
  add("prompt-answer-field-never-opens", [
    ...promptWorker(),
    captureAndSave,
    fake((context) => {
      (context as unknown as { script: PromptScript }).script.tabDoesNothing =
        true;
    }),
    answerWith({ kind: "text", number: 4, text: "nope" }),
  ]);
  add("prompt-answer-text-not-echoed", [
    ...promptWorker(),
    captureAndSave,
    fake((context) => {
      context.fake.onText = undefined;
    }),
    answerWith({ kind: "text", number: 4, text: "nope" }),
  ]);
  add("prompt-answer-screen-changes", [
    ...promptWorker(),
    captureAndSave,
    fake((context) => {
      const script = (context as unknown as { script: PromptScript }).script;
      const original = context.fake.onKey!;
      let seen = 0;
      context.fake.onKey = (target, key) => {
        original(target, key);
        seen += 1;
        if (seen === 1) target.screen = script.render().replace("Bash", "Bosh");
      };
    }),
    answerWith({ kind: "option", number: 3 }),
  ]);
  add("prompt-answer-before-type-throws", [
    ...promptWorker(),
    captureAndSave,
    op("answerPrompt", (context) => ({
      paneId: context.vars.pane!,
      promptSha: context.vars.sha!,
      answer: { kind: "option", number: 2 },
      beforeType: "throw",
    })),
  ]);
  add("prompt-answer-unrecognized", [
    ...promptWorker(),
    captureAndSave,
    screen(idle()),
    answerWith({ kind: "option", number: 2 }),
    ...startWorker(() => idle(), ["blocked"], "omp", "ompdev"),
    answerWith({ kind: "esc" }, "0".repeat(64)),
  ]);
  const dialogSteps = (status: string): Step[] => [
    ...startWorker(
      () => promptFixture("synthetic-dialog-teach-auto-mode.ansi"),
      [status],
    ),
    captureAndSave,
  ];
  add("dialog-capture-and-esc", [
    ...dialogSteps("idle"),
    fake((context) => {
      context.fake.onKey = (target, key) => {
        if (key === "esc") target.screen = idle();
      };
    }),
    answerWith({ kind: "esc" }),
  ]);
  add("dialog-esc-input-never-readable", [
    ...dialogSteps("idle"),
    answerWith({ kind: "esc" }),
  ]);
  add("dialog-working", [
    ...dialogSteps("working"),
    answerWith({ kind: "esc" }),
  ]);
  add("dialog-refuses-options", [
    ...dialogSteps("idle"),
    answerWith({ kind: "option", number: 1 }),
    answerWith({ kind: "esc" }, "0".repeat(64)),
    screen(idle()),
    answerWith({ kind: "esc" }),
  ]);

  // ---- interrupt
  add("interrupt-working", [
    ...startWorker(() => idle(), ["working"]),
    op("interruptWorking", onPane()),
    agentIs("idle"),
    op("interruptWorking", onPane()),
    op("interruptWorking", { paneId: "w44:p1" }),
  ]);

  // ---- pane operations
  add("pane-operations", [
    ...startWorker(),
    op(
      "createTab",
      {
        workspaceId: "w9",
        cwd: "/tmp",
        label: "tab",
        role: "worker",
      },
      saveAs("tabPane", "paneId"),
    ),
    op(
      "createWorkspace",
      { cwd: "/tmp", label: "proj", role: "PM" },
      saveAs("pm", "paneId"),
    ),
    op("paneEntry", (context) => ({ paneId: context.vars.pm! })),
    fake((context) => {
      context.fake.layoutRects = [
        { pane_id: context.vars.pm!, rect: { width: 100, height: 40 } },
        { pane_id: context.vars.pane!, rect: { width: "x", height: 20 } },
      ];
    }),
    op("paneLayout", (context) => ({ paneId: context.vars.pm! })),
    fake((context) => {
      context.fake.zoomed = true;
    }),
    op("paneLayout", (context) => ({ paneId: context.vars.pm! })),
    op("panesAtPath", (context) => ({
      directory: pane(context).checkout,
    })),
    op("panesAtPath", (context) => ({
      directory: `${pane(context).checkout}/`,
    })),
    op("panesAtPath", (context) => ({
      directory: `${pane(context).checkout}/sub/..`,
    })),
    op("panesAtPath", { directory: "relative/dir" }),
    op("panesAtPath", { directory: "/nowhere/at/all" }),
    op("paneIdentity", onPane()),
    op("paneIdentity", { paneId: "w88:p1" }),
    fake((context) => {
      const target = pane(context);
      target.terminalId = "t-1";
      target.tokens = { agent: "dev", project: "", other: "x" };
    }),
    op("paneIdentity", onPane()),
    fake((context) => {
      pane(context).tokens = "not an object" as unknown as Record<
        string,
        unknown
      >;
    }),
    op("paneIdentity", onPane()),
    op(
      "reportMetadata",
      onPane({ tokens: { project: "acme", agent: "dev" } }),
      undefined,
    ),
    op(
      "reportMetadata",
      onPane({ tokens: { project: "acme", role: "developer", agent: "dev" } }),
    ),
    op("reportMetadata", { workspaceId: "w9", tokens: { agent: "dev" } }),
    op("reportMetadata", onPane({ tokens: { Bad: "x" } })),
    op("reportMetadata", onPane({ tokens: { agent: "bad\nlabel" } })),
    op("renameTab", { tabId: "w9:t1", label: "name" }),
    op("renameTab", { tabId: "bad", label: "name" }),
    op("renameTab", { tabId: "w9:t1", label: "" }),
    op("renameWorkspace", { workspaceId: "w9", label: "name" }),
    op("renameWorkspace", { workspaceId: "9", label: "name" }),
    op("closePane", (context) => ({ paneId: context.vars.tabPane! })),
    op("paneEntry", (context) => ({ paneId: context.vars.tabPane! })),
    op("removeWorktree", { workspaceId: "w9", force: true }),
    op("removeWorktree", { workspaceId: "w1", force: false }),
    op("removeWorktree", { workspaceId: "bad" }),
  ]);
  add("remove-worktree-pm-workspace", [
    ...startPm(),
    op("removeWorktree", { workspaceId: "w1" }),
  ]);
  for (const mode of [
    "ok",
    "error-no-move",
    "error-moved",
    "error-lost",
  ] as const)
    add(`place-pane-${mode}`, [
      ...startPm(),
      ...startWorker(),
      fake((context) => {
        context.fake.moveMode = mode;
      }),
      place(),
      op("paneEntry", onPane()),
    ]);
  add("place-pane-ambiguous", [
    ...startPm(),
    ...startWorker(),
    fake((context) => {
      context.fake.moveMode = "error-moved";
      context.fake.extraPaneAtMovedPath = true;
    }),
    place({ direction: "down", keep: 0.25 }),
  ]);
  add("place-pane-list-fails", [
    ...startPm(),
    ...startWorker(),
    fake((context) => {
      context.fake.moveMode = "error-moved";
      context.fake.failListCall = 2;
    }),
    place(),
  ]);
  add("place-pane-validation", [
    ...startPm(idle(), "pm"),
    ...startWorker(),
    fake((context) => {
      context.vars.checkout = pane(context).checkout;
    }),
    ...[
      { keep: 0.05 },
      { keep: 0.95 },
      { tabId: "bad" },
      { targetPaneId: "bad" },
      { direction: "left" },
      { worktreePath: "relative" },
      { paneId: "w61:p1" },
      { keep: 0.1 },
      { keep: 0.9 },
    ].map((override) =>
      op("placePane", (context) => ({
        paneId: context.vars.pane!,
        tabId: "w1:t1",
        targetPaneId: "w1:p1",
        direction: "right",
        keep: 0.5,
        worktreePath: context.vars.checkout!,
        ...override,
      })),
    ),
    op("placePane", (context) => ({
      paneId: context.vars.pm!,
      tabId: "w1:t1",
      targetPaneId: "w1:p1",
      direction: "right",
      keep: 0.5,
      worktreePath: "/tmp",
    })),
  ]);
  add("pane-list-odd-entries", [
    ...startWorker(),
    fake((context) => {
      context.fake.extraListEntries = [
        "junk",
        { pane_id: "bad" },
        {
          pane_id: "w5:p1",
          tab_id: "w5:t1",
          workspace_id: "w5",
          cwd: "relative",
        },
        {
          pane_id: "w6:p1",
          tab_id: "w6:t1",
          workspace_id: "w6",
          cwd: pane(context).checkout,
        },
        {
          pane_id: "w7:p1",
          tab_id: "bad",
          workspace_id: "w7",
          cwd: pane(context).checkout,
        },
      ];
    }),
    op("panesAtPath", (context) => ({ directory: pane(context).checkout })),
  ]);
  add("create-operations-validation", [
    op("createWorktree", { workspaceId: "bad", branch: "b", label: "l" }),
    op("createWorktree", { workspaceId: "w1", branch: "-b", label: "l" }),
    op("createWorktree", { workspaceId: "w1", branch: "b", label: " " }),
    op("createWorktree", {
      workspaceId: "w1",
      branch: "b",
      label: "l",
      base: "../x",
    }),
    op("createWorktree", {
      workspaceId: "w1",
      branch: "b",
      label: "l",
      base: "main",
    }),
    op("createWorkspace", { cwd: "relative", label: "l", role: "worker" }),
    op("createWorkspace", { cwd: "/tmp", label: "", role: "worker" }),
    op("createTab", {
      workspaceId: "bad",
      cwd: "/tmp",
      label: "l",
      role: "worker",
    }),
    op("createTab", {
      workspaceId: "w1",
      cwd: "rel",
      label: "l",
      role: "worker",
    }),
    op("createTab", {
      workspaceId: "w1",
      cwd: "/tmp",
      label: "\n",
      role: "worker",
    }),
  ]);
  add("adopt-pane", [
    ...startWorker(),
    agentIs("working"),
    op("forgetPane", onPane()),
    op(
      "adoptPane",
      onPane({
        role: "worker",
        agent: "dev",
        workspaceId: "w9",
        worktreePath: "/tmp/x",
      }),
    ),
    op("paneEntry", onPane()),
    adopt(),
    op("forgetPane", onPane()),
    adopt({ role: "PM" }),
    op("paneEntry", onPane()),
    op("adoptPane", {
      paneId: "w70:p1",
      role: "worker",
      agent: "dev",
      workspaceId: null,
      worktreePath: null,
    }),
    op("adoptPane", {
      paneId: "bad",
      role: "worker",
      agent: "dev",
      workspaceId: null,
      worktreePath: null,
    }),
    op(
      "adoptPane",
      onPane({
        role: "worker",
        agent: "bad name",
        workspaceId: null,
        worktreePath: null,
      }),
    ),
    op("forgetPane", onPane()),
    op(
      "adoptPane",
      onPane({
        role: "worker",
        agent: "ghost",
        workspaceId: null,
        worktreePath: null,
      }),
    ),
    fake((context) => {
      setAgent(context, "dev", {
        paneId: "w9:p9",
        statuses: ["idle"],
      });
    }),
    adopt(),
    fake((context) => {
      setAgent(context, "dev", {
        paneId: context.vars.pane!,
        statuses: ["idle"],
        kind: "vim",
      });
    }),
    adopt(),
    op("adoptShellPane", onPane({ workspaceId: "w9" })),
    op("paneEntry", onPane()),
    op("adoptShellPane", onPane({ workspaceId: null })),
    op("adoptShellPane", { paneId: "w71:p1", workspaceId: null }),
    op("adoptShellPane", { paneId: "bad", workspaceId: null }),
  ]);
  add(
    "adopt-legacy-named-agent",
    [
      ...startWorker(),
      fake((context) => {
        context.fake.agentStates.delete("acme-dev");
        context.fake.agentStates.set("dev", {
          paneId: context.vars.pane!,
          statuses: ["idle"],
        });
      }),
      op("forgetPane", onPane()),
      adopt(),
      op("paneEntry", onPane()),
      op("forgetPane", onPane()),
      fake((context) => {
        context.fake.agentStates.delete("acme-dev");
        context.fake.agentStates.set("dev", {
          paneId: "w9:p9",
          statuses: ["idle"],
        });
      }),
      adopt(),
    ],
    "acme",
  );
  add("notify", [
    op("notify", {
      title: "Capstan: PM message waiting",
      body: "Message m-1 is waiting",
    }),
    fake((context) => {
      context.fake.notification = {
        shown: false,
        reason: "disabled!! by_user",
      };
    }),
    op("notify", { title: "t", body: "b" }),
    fake((context) => {
      context.fake.notification = { shown: false };
    }),
    op("notify", { title: "t", body: "b" }),
    fake((context) => {
      context.fake.notification = { shown: "yes" };
    }),
    op("notify", { title: "t", body: "b" }),
    op("notify", { title: "", body: "b" }),
    op("notify", { title: "t".repeat(101), body: "b" }),
    op("notify", { title: "t", body: "b".repeat(501) }),
    op("notify", { title: "t", body: "line\nbreak" }),
    op("notify", { title: "é".repeat(100), body: "é".repeat(500) }),
  ]);
  add("version-failure", [
    fake((context) => {
      context.overrides.push({
        match: (args) => args[0] === "--version",
        response: { code: 2, stdout: "", stderr: "bad" },
      });
    }),
    op("version"),
  ]);
  add("bad-output-shapes", [
    op(
      "createWorkspace",
      { cwd: "/tmp", label: "x", role: "worker" },
      saveAs("pane", "paneId"),
    ),
    ...[
      ["pane", "get"],
      ["agent", "get"],
      ["pane", "list"],
      ["pane", "layout"],
    ].map(([a, b]) =>
      fake((context) => {
        context.overrides.push({
          match: (args) => args[0] === a && args[1] === b,
          response: {
            code: 0,
            stdout: '{"result":{"unexpected":true}}',
            stderr: "",
          },
        });
      }),
    ),
    op("paneState", onPane()),
    op("agentState", { name: "dev" }),
    op("panesAtPath", { directory: "/tmp" }),
    op("paneLayout", onPane()),
    fake((context) => {
      context.overrides.push({
        match: (args) => args[0] === "pane" && args[1] === "read",
        response: { code: 1, stdout: "", stderr: "plain failure" },
      });
    }),
    op("readScreen", onPane()),
    op("readInput", onPane()),
    op("readInput", { paneId: "w99:p1" }),
    op("readScreen", { paneId: "bad" }),
  ]);
  return list;
}

// -- running a scenario with the Node adapter ------------------------------------------------------------------------

function normalise(value: Json, rootPath: string): Json {
  if (typeof value === "string")
    return value
      .replaceAll(rootPath, "<root>")
      .replace(/capstan-shell-[A-Za-z0-9]{6}/g, "capstan-shell-XXXXXX");
  if (Array.isArray(value)) return value.map((v) => normalise(v, rootPath));
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([k, v]) => [k, normalise(v, rootPath)]),
    );
  return value;
}

function directoriesUnder(rootPath: string): string[] {
  const found: string[] = [];
  const walk = (directory: string): void => {
    for (const item of readdirSync(directory, { withFileTypes: true })) {
      if (!item.isDirectory()) continue;
      if (/^capstan-(?:shell|prompts)-/.test(item.name)) continue;
      const full = path.join(directory, item.name);
      found.push(path.relative(rootPath, full));
      walk(full);
    }
  };
  walk(rootPath);
  return found.sort();
}

function errorJson(error: unknown): Json {
  if (!(error instanceof Error))
    return { name: "Error", message: String(error) };
  const extra = error as Error & { code?: unknown; blocker?: unknown };
  return {
    name: error.name,
    message: error.message,
    ...(typeof extra.code === "string" ? { code: extra.code } : {}),
    ...(typeof extra.blocker === "string" ? { blocker: extra.blocker } : {}),
  };
}

type Adapter = HerdrAdapter;

async function perform(
  adapter: Adapter,
  name: string,
  args: Record<string, Json>,
  events: string[],
): Promise<unknown> {
  const s = (key: string): string => args[key] as string;
  const opt = (key: string): string | undefined =>
    args[key] === undefined || args[key] === null
      ? undefined
      : (args[key] as string);
  const nullable = (key: string): string | null =>
    args[key] === undefined ? null : (args[key] as string | null);
  const role = args.role as "PM" | "worker";
  const hook = (key: string, label: string) => (): void => {
    events.push(label);
    if (args[key] === "throw") throw new StaleActionError("stale");
  };
  const log = (entry: { key: string; reason: string }): void => {
    events.push(`key:${entry.key}`);
  };
  switch (name) {
    case "version":
      return adapter.version();
    case "createWorktree":
      return adapter.createWorktree({
        workspaceId: s("workspaceId"),
        branch: s("branch"),
        label: s("label"),
        ...(opt("base") === undefined ? {} : { base: s("base") }),
      });
    case "createWorkspace":
      return adapter.createWorkspace({
        cwd: s("cwd"),
        label: s("label"),
        role,
      });
    case "createTab":
      return adapter.createTab({
        workspaceId: s("workspaceId"),
        cwd: s("cwd"),
        label: s("label"),
        role,
      });
    case "startAgent":
      return adapter.startAgent({
        name: s("name"),
        kind: s("kind"),
        paneId: s("paneId"),
        args: args.args as string[],
        ...(args.timeoutMs === undefined
          ? {}
          : { timeoutMs: args.timeoutMs as number }),
        ...(args.environment === undefined
          ? {}
          : { environment: args.environment as Record<string, string> }),
      });
    case "prepareShell":
      return adapter.prepareShell({
        paneId: s("paneId"),
        environment: args.environment as Record<string, string>,
        ...(args.timeoutMs === undefined
          ? {}
          : { timeoutMs: args.timeoutMs as number }),
      });
    case "runInPane":
      return adapter.runInPane(s("paneId"), s("command"));
    case "guardedSend":
      return adapter.guardedSend({
        paneId: s("paneId"),
        text: s("text"),
        beforeSend: hook("beforeSend", "beforeSend"),
      });
    case "wakePm":
      return adapter.wakePm({
        paneId: s("paneId"),
        text: s("text"),
        beforeSend: hook("beforeSend", "beforeSend"),
      });
    case "clearAfterDeferral":
      return adapter.clearAfterDeferral({
        paneId: s("paneId"),
        deferredForMs: args.deferredForMs as number,
        maxDeferralMs: args.maxDeferralMs as number,
        discard: (value) => {
          events.push(`discard:${value}`);
          if (args.discard === "throw") throw new StaleActionError("stale");
        },
        log,
      });
    case "answerTrustDialog":
      return adapter.answerTrustDialog({
        paneId: s("paneId"),
        log,
        ...(args.timeoutMs === undefined
          ? {}
          : { timeoutMs: args.timeoutMs as number }),
      });
    case "capturePrompt":
      return adapter.capturePrompt(s("paneId"));
    case "answerPrompt":
      return adapter.answerPrompt({
        paneId: s("paneId"),
        promptSha: s("promptSha"),
        answer: args.answer as unknown as PromptAnswer,
        beforeType: hook("beforeType", "beforeType"),
        log,
      });
    case "interruptWorking":
      return adapter.interruptWorking({ paneId: s("paneId"), log });
    case "readScreen":
      return adapter.readScreen(s("paneId"), {
        ...(args.ansi === true ? { ansi: true } : {}),
        ...(args.lines === undefined ? {} : { lines: args.lines as number }),
      });
    case "readInput":
      return adapter.readInput(s("paneId"));
    case "agentState":
      return adapter.agentState(s("name"));
    case "paneState":
      return adapter.paneState(s("paneId"));
    case "paneEntry":
      return adapter.paneEntry(s("paneId"));
    case "paneForAgent":
      return adapter.paneForAgent(s("agentId"));
    case "agentObservation":
      return adapter.agentObservation(s("agentId"));
    case "forgetPane":
      return adapter.forgetPane(s("paneId"));
    case "notify":
      return adapter.notify(s("title"), s("body"));
    case "paneLayout":
      return adapter.paneLayout(s("paneId"));
    case "panesAtPath":
      return adapter.panesAtPath(s("directory"));
    case "placePane":
      return adapter.placePane({
        paneId: s("paneId"),
        tabId: s("tabId"),
        targetPaneId: s("targetPaneId"),
        direction: s("direction") as "right" | "down",
        keep: args.keep as number,
        worktreePath: s("worktreePath"),
      });
    case "paneIdentity":
      return adapter.paneIdentity(s("paneId"));
    case "closePane":
      return adapter.closePane(s("paneId"));
    case "removeWorktree":
      return adapter.removeWorktree(s("workspaceId"), {
        force: args.force === true,
      });
    case "reportMetadata":
      return adapter.reportMetadata(
        args.paneId === undefined
          ? { workspaceId: s("workspaceId") }
          : { paneId: s("paneId") },
        args.tokens as Record<string, string>,
      );
    case "renameTab":
      return adapter.renameTab(s("tabId"), s("label"));
    case "renameWorkspace":
      return adapter.renameWorkspace(s("workspaceId"), s("label"));
    case "adoptPane":
      return adapter.adoptPane({
        paneId: s("paneId"),
        role,
        agent: s("agent"),
        workspaceId: nullable("workspaceId"),
        worktreePath: nullable("worktreePath"),
      });
    case "adoptShellPane":
      return adapter.adoptShellPane(s("paneId"), nullable("workspaceId"));
    default:
      throw new Error(`unknown op ${name}`);
  }
}

async function recordScenario(scenario: Scenario): Promise<Json> {
  const harness = new FakeHerdr();
  const rootPath = harness.root;
  const calls: Json[] = [];
  let clock = 0;
  const context: Context = {
    slug: scenario.slug,
    fake: harness,
    vars: {},
    overrides: [],
  };
  const run = async (args: readonly string[]): Promise<HerdrResult> => {
    const overrideAt = context.overrides.findIndex((o) => o.match(args));
    let response: HerdrResult;
    if (overrideAt >= 0) {
      harness.calls.push([...args]);
      response = context.overrides.splice(overrideAt, 1)[0]!.response;
    } else response = await harness.run(args);
    // Whether starting the prepared shell removed its private directory (the fake's `onRun`).
    const rcFile = /--rcfile '([^']+)'/.exec(args[3] ?? "")?.[1];
    calls.push({
      argv: [...args],
      response: { ...response },
      dirs: directoriesUnder(rootPath),
      rcRemoved: rcFile !== undefined && !existsSync(path.dirname(rcFile)),
    });
    return response;
  };
  const adapter = new HerdrAdapter({
    ...(scenario.slug === undefined ? {} : { projectSlug: scenario.slug }),
    run,
    tempRoot: rootPath,
    sleep: async () => {
      clock += 100;
    },
    now: () => clock,
    pollMs: 100,
  });
  const steps: Json[] = [];
  try {
    for (const step of scenario.steps) {
      if ("fake" in step) {
        step.fake(context);
        continue;
      }
      const args = step.args(context);
      const events: string[] = [];
      calls.length = 0;
      let outcome: Json;
      try {
        const value = await perform(adapter, step.op, args, events);
        outcome = {
          ok:
            value === undefined
              ? null
              : (JSON.parse(JSON.stringify(value)) as Json),
        };
      } catch (error) {
        outcome = { error: errorJson(error) };
      }
      step.save?.(outcome, context);
      steps.push(
        normalise(
          { op: step.op, args, outcome, events, calls: [...calls], clock },
          rootPath,
        ),
      );
    }
  } finally {
    adapter.close();
    harness.cleanup();
    rmSync(rootPath, { recursive: true, force: true });
  }
  return {
    name: scenario.name,
    slug: orNull(scenario.slug),
    steps,
  };
}

async function adapterCases(): Promise<Json> {
  const recorded: Json[] = [];
  for (const scenario of scenarios())
    recorded.push(await recordScenario(scenario));
  return recorded;
}

/** Every fixture file by name, as the text the exporter writes. */
export async function exportFixtures(): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  files.set("screen.json", JSON.stringify(screenCases()) + "\n");
  files.set("relay.json", JSON.stringify(relayCases()) + "\n");
  files.set("validate.json", JSON.stringify(validateCases()) + "\n");
  files.set("runner.json", JSON.stringify(await runnerCases()) + "\n");
  files.set("process.json", JSON.stringify(await processCases()) + "\n");
  files.set("adapter.json", JSON.stringify(await adapterCases()) + "\n");
  return files;
}

if (import.meta.filename === process.argv[1]) {
  mkdirSync(PARITY_DIRECTORY, { recursive: true });
  const files = await exportFixtures();
  for (const [name, content] of files)
    writeFileSync(path.join(PARITY_DIRECTORY, name), content);
  process.stdout.write(`wrote ${files.size} fixtures to ${PARITY_DIRECTORY}\n`);
}

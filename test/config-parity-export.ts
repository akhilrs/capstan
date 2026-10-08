/**
 * Writes the fixtures the Rust configuration reader (rust/crates/config) is tested against: what this Node
 * implementation makes of the cases in config-corpus.json and prompts-corpus.json (plus the cases built below), and the
 * seeded generator of the differential TOML run.
 *
 *   npm run build && node dist/test/config-parity-export.js                 rewrites the generated cases and the expectations (config-edge-cases, prompts-built, config-expected, prompts-expected)
 *   node dist/test/config-parity-export.js --show-config|--show-prompt <name>      prints what Node makes of one case
 *   node dist/test/config-parity-export.js --differential <count> <seed> <file>   writes the differential cases (JSON lines);
 *       then CAPSTAN_CONFIG_DIFFERENTIAL=<file> cargo test -p capstan-config --test differential -- --nocapture compares Rust with them
 *   node dist/test/config-parity-export.js --print-capture-hooks configs|prompts > hooks.mjs
 *       then run the existing tests with the loaders wrapped, CAPTURE_FILE set, and fold what they saw into a corpus:
 *       node --import "data:text/javascript,import {register} from 'node:module'; register('file://$PWD/hooks.mjs')" --test dist/test/config.test.js ...
 *       node dist/test/config-parity-export.js --fold-configs|--fold-prompts $CAPTURE_FILE
 *
 * config-parity.test.ts fails while the committed files differ from a fresh export.
 */
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  ConfigError,
  loadCapstanConfig,
  parseCapstanConfig,
} from "../src/config/capstan-config.js";
import { buildRolePrompt, type PromptInput } from "../src/prompts.js";
import { STARTER_CONFIG } from "../src/config/starter.js";
import { DESIGNER_PROMPT } from "../src/roles/designer-prompt.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const PARITY_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "config",
  "tests",
  "parity",
);
const CONFIG_CORPUS = "config-corpus.json";
const PROMPT_CORPUS = "prompts-corpus.json";
const CONFIG_EXPECTED = "config-expected.json";
const PROMPT_EXPECTED = "prompts-expected.json";
const EDGE_CASES = "config-edge-cases.json";
const PROMPT_BUILT = "prompts-built.json";
const DIFFERENTIAL = "differential.json";
/** The seed and size of the differential run recorded in differential.json; larger runs use other seeds. */
export const DIFFERENTIAL_SEED = 20261008;
export const DIFFERENTIAL_COUNT = 20000;

/** One file, directory or link of a scratch project. `$ROOT` in `link` stands for the project directory. */
export interface CorpusEntry {
  readonly path: string;
  readonly text?: string;
  /** The text of the starter configuration or the designer prompt, which many cases hold; the Rust side reads its own copy. */
  readonly ref?: "starter" | "designer";
  readonly base64?: string;
  readonly mode?: number;
  readonly link?: string;
  readonly dir?: true;
}

export interface CorpusCase {
  readonly name: string;
  readonly entries: readonly CorpusEntry[];
}

export type Outcome =
  | { readonly kind: "ok"; readonly json: string }
  | { readonly kind: "error"; readonly message: string }
  | { readonly kind: "parse-error"; readonly message: string }
  | { readonly kind: "unexpected"; readonly message: string };

function readJson<T>(name: string): T {
  return JSON.parse(
    readFileSync(path.join(PARITY_DIRECTORY, name), "utf8"),
  ) as T;
}

/** Makes the project the case describes under `directory`. */
export function materialize(
  entries: readonly CorpusEntry[],
  directory: string,
): void {
  for (const entry of entries) {
    const target = path.join(directory, entry.path);
    if (entry.dir === true) mkdirSync(target, { recursive: true });
    else if (entry.link !== undefined)
      symlinkSync(entry.link.split("$ROOT").join(directory), target);
    else {
      mkdirSync(path.dirname(target), { recursive: true });
      writeFileSync(
        target,
        entry.base64 !== undefined
          ? Buffer.from(entry.base64, "base64")
          : entry.ref === "starter"
            ? STARTER_CONFIG
            : entry.ref === "designer"
              ? DESIGNER_PROMPT
              : (entry.text ?? ""),
      );
      chmodSync(target, entry.mode ?? 0o600);
    }
  }
}

const TOML_ERROR = /^capstan\.toml is not valid TOML at line \d+, column \d+$/;

/** A committed outcome keeps the SHA-256 and length of the JSON, not the JSON (`--show-config <name>` prints it). */
export type Expected =
  | { readonly kind: "ok"; readonly sha256: string; readonly bytes: number }
  | Exclude<Outcome, { kind: "ok" }>;

export function shrink(outcome: Outcome): Expected {
  if (outcome.kind !== "ok") return outcome;
  return {
    kind: "ok",
    sha256: createHash("sha256").update(outcome.json, "utf8").digest("hex"),
    bytes: Buffer.byteLength(outcome.json, "utf8"),
  };
}

/** What the Node loader makes of one scratch project. */
export function evaluate(entries: readonly CorpusEntry[]): Outcome {
  const directory = realpathSync(
    mkdtempSync(path.join(os.tmpdir(), "cstan-cfgparity-")),
  );
  const restore = (text: string): string => text.split(directory).join("$ROOT");
  try {
    materialize(entries, directory);
    try {
      return {
        kind: "ok",
        json: restore(JSON.stringify(loadCapstanConfig(directory), null, 2)),
      };
    } catch (error) {
      const message = restore(
        error instanceof Error ? error.message : String(error),
      );
      if (!(error instanceof ConfigError))
        return { kind: "unexpected", message };
      return {
        kind: TOML_ERROR.test(message) ? "parse-error" : "error",
        message,
      };
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const file = (
  text: string,
  name = "capstan.toml",
  mode = 0o600,
): CorpusEntry => ({
  path: name,
  text,
  mode,
});

const VALID = `schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"
`;

/** TOML edge cases the Node tests do not spell out. */
function edgeCases(): CorpusCase[] {
  // prettier-ignore
  const cases: [string, string | Buffer][] = [
    ["edge-valid", VALID],
    ["edge-oversize", `# ${"a".repeat(64 * 1024)}\n${VALID}`],
    [ "edge-exactly-max", `${VALID}#${"a".repeat(64 * 1024 - VALID.length - 1)}`, ],
    ["edge-empty", ""],
    ["edge-bom", `\uFEFF${VALID}`],
    ["edge-double-bom", `\uFEFF\uFEFF${VALID}`],
    ["edge-crlf", VALID.replace(/\n/g, "\r\n")],
    ["edge-lone-cr", VALID.replace("\n\n", "\n\r\n").replace('pm"\n', 'pm"\r')],
    ["edge-invalid-utf8", Buffer.from([0x73, 0x63, 0xff, 0xfe])],
    ["edge-nul", `${VALID}\u0000`],
    [ "edge-big-integer", VALID.replace("schema_version = 1", "schema_version = 99999999999999999999",), ],
    [ "edge-big-integer-limit", `${VALID}\n[limits]\nmax_workers = 9223372036854775808\n`, ],
    ["edge-i64-max", `${VALID}\n[limits]\nmax_workers = 9223372036854775807\n`],
    [ "edge-negative-integer", `${VALID}\n[limits]\nmax_workers = -9223372036854775808\n`, ],
    ["edge-hex-integer", `${VALID}\n[limits]\nmax_workers = 0x4\n`],
    ["edge-octal-integer", `${VALID}\n[limits]\nmax_workers = 0o4\n`],
    ["edge-binary-integer", `${VALID}\n[limits]\nmax_workers = 0b100\n`],
    ["edge-underscore-integer", `${VALID}\n[limits]\nmax_workers = 1_0\n`],
    ["edge-leading-zero-integer", `${VALID}\n[limits]\nmax_workers = 04\n`],
    ["edge-plus-integer", `${VALID}\n[limits]\nmax_workers = +4\n`],
    ["edge-float-integer", `${VALID}\n[limits]\nmax_workers = 4.0\n`],
    ["edge-exponent-float", `${VALID}\n[limits]\nmax_workers = 4e0\n`],
    ["edge-nan", `${VALID}\n[limits]\nmax_workers = nan\n`],
    ["edge-inf", `${VALID}\n[limits]\nmax_workers = inf\n`],
    ["edge-date", `${VALID}\n[limits]\nmax_workers = 1979-05-27\n`],
    [ "edge-datetime", `${VALID}\n[limits]\nmax_workers = 1979-05-27T07:32:00Z\n`, ],
    ["edge-local-time", `${VALID}\n[limits]\nmax_workers = 07:32:00\n`],
    ["edge-short-time", `${VALID}\n[limits]\nmax_workers = 07:32\n`],
    [ "edge-short-datetime", `${VALID}\n[limits]\nmax_workers = 1979-05-27T07:32\n`, ],
    [ "edge-dotted-keys", `schema_version = 1\nhosts.claude.kind = "claude"\nroles.pm.kind = "PM"\nroles.pm.host = "claude"\n`, ],
    [ "edge-dotted-then-table", `${VALID}\nlimits.max_workers = 2\n[limits]\nmax_workers = 3\n`, ],
    [ "edge-table-then-dotted", `schema_version = 1\n[hosts]\nclaude.kind = "claude"\n[roles]\npm = { kind = "PM", host = "claude" }\n`, ],
    [ "edge-inline-tables", `schema_version = 1\nhosts = { claude = { kind = "claude" } }\nroles = { pm = { kind = "PM", host = "claude" } }\n`, ],
    [ "edge-inline-newline", `schema_version = 1\nhosts = { claude = { kind = "claude" } }\nroles = {\n  pm = { kind = "PM",\n host = "claude" }\n}\n`, ],
    [ "edge-inline-trailing-comma", `schema_version = 1\nhosts = { claude = { kind = "claude", } }\nroles = { pm = { kind = "PM", host = "claude" } }\n`, ],
    [ "edge-inline-extend", `schema_version = 1\nhosts = { claude = { kind = "claude" } }\n[hosts.claude]\ncommand = "claude"\n[roles.pm]\nkind = "PM"\nhost = "claude"\n`, ],
    ["edge-duplicate-key", `${VALID}\nschema_version = 1\n`],
    ["edge-duplicate-table", `${VALID}\n[hosts.claude]\nkind = "claude"\n`],
    [ "edge-duplicate-in-inline", `schema_version = 1\nhosts = { claude = { kind = "claude", kind = "codex" } }\nroles = { pm = { kind = "PM", host = "claude" } }\n`, ],
    ["edge-array-of-tables", `${VALID}\n[[mcp_servers]]\nname = "x"\n`],
    ["edge-array-in-array", `${VALID}\n[env]\npass = [["A"]]\n`],
    ["edge-mixed-array", `${VALID}\n[env]\npass = ["A", 1]\n`],
    ["edge-array-trailing-comma", `${VALID}\n[env]\npass = ["A",]\n`],
    [ "edge-array-comments", `${VALID}\n[env]\npass = [\n  "A", # one\n  "B",\n]\n`, ],
    [ "edge-unicode-role-name", `${VALID}\n[roles."ünï"]\nkind = "Developer"\nhost = "claude"\n`, ],
    [ "edge-unicode-host-name", `schema_version = 1\n[hosts."ホスト"]\nkind = "claude"\n[roles.pm]\nkind = "PM"\nhost = "ホスト"\n`, ],
    [ "edge-quoted-keys", `"schema_version" = 1\n["hosts"."claude"]\n'kind' = "claude"\n["roles"."pm"]\nkind = "PM"\nhost = "claude"\n`, ],
    ["edge-empty-key", `${VALID}\n"" = 1\n`],
    [ "edge-numeric-keys", `${VALID}\n[roles.1]\nkind = "Developer"\nhost = "claude"\n`, ],
    [ "edge-numeric-order", `schema_version = 1\n[hosts.claude]\nkind = "claude"\n[roles.zeta]\nkind = "PM"\nhost = "claude"\n[roles.10]\nkind = "Developer"\nhost = "claude"\n[roles.2]\nkind = "Developer"\nhost = "claude"\n`, ],
    ["edge-proto-key", `${VALID}\n[mcp_servers.__proto__]\ncommand = "x"\n`],
    ["edge-proto-dotted", `${VALID}\n__proto__.x = 1\n`],
    [ "edge-literal-string", `schema_version = 1\n[hosts.claude]\nkind = 'claude'\n[roles.pm]\nkind = 'PM'\nhost = 'claude'\n`, ],
    [ "edge-multiline-string", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = """\nline one\nline two\n"""\n\n[roles.reviewer]')}`, ],
    [ "edge-multiline-literal", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', "host = \"claude\"\nprompt = '''\nback\\slash\n'''\n\n[roles.reviewer]")}`, ],
    [ "edge-line-continuation", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = """a \\\n   b"""\n\n[roles.reviewer]')}`, ],
    [ "edge-escape-unicode", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "caf\\u00e9 \\U0001F600"\n\n[roles.reviewer]')}`, ],
    [ "edge-escape-hex", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "\\xE9"\n\n[roles.reviewer]')}`, ],
    [ "edge-escape-esc", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "a\\eb"\n\n[roles.reviewer]')}`, ],
    [ "edge-escape-bad", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "\\q"\n\n[roles.reviewer]')}`, ],
    [ "edge-escape-surrogate", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "\\uD800"\n\n[roles.reviewer]')}`, ],
    [ "edge-escape-too-big", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "\\U00110000"\n\n[roles.reviewer]')}`, ],
    [ "edge-control-in-string", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "a\u0001b"\n\n[roles.reviewer]')}`, ],
    [ "edge-del-in-string", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "a\u007fb"\n\n[roles.reviewer]')}`, ],
    [ "edge-tab-in-string", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "a\tb"\n\n[roles.reviewer]')}`, ],
    ["edge-comment-control", `${VALID}# note \u0001\n`],
    ["edge-comment-del", `${VALID}# note \u007f\n`],
    ["edge-unterminated-string", `${VALID}\n[project]\nname = "x\n`],
    ["edge-unterminated-table", `${VALID}\n[project\n`],
    ["edge-bare-key-chars", `${VALID}\n[project]\nna.me = "x"\n`],
    ["edge-no-newline-end", VALID.trimEnd()],
    ["edge-tabs", VALID.replace(/\n/g, "\n\t")],
    ["edge-vertical-tab", `${VALID}\u000b`],
    ["edge-nbsp", `${VALID}\u00a0`],
    [ "edge-secret-in-name", `schema_version = 1\n[hosts.claude]\nkind = "claude"\n[roles.pm]\nkind = "PM"\nhost = "claude"\n[roles.sk-abcdefgh1234]\nkind = "Developer"\nhost = "claude"\n`, ],
    [ "edge-numeric-and-secret-names", `schema_version = 1\n[hosts.claude]\nkind = "claude"\n[roles.sk-abcdefgh1234]\nkind = "PM"\nhost = "claude"\n[roles.1]\nkind = "Developer"\nhost = "claude"\n`, ],
    [ "edge-supplementary-length", `${VALID}\n[project]\nname = "${"😀".repeat(129)}"\n`, ],
    ["edge-project-name-bidi", `${VALID}\n[project]\nname = "a\u202eb"\n`],
    ["edge-project-name-zwj", `${VALID}\n[project]\nname = "a\u200db"\n`],
    ["edge-project-name-tag", `${VALID}\n[project]\nname = "a\u{e0001}b"\n`],
    ["edge-project-name-nbsp", `${VALID}\n[project]\nname = "a\u00a0b"\n`],
    ["edge-project-name-edge-space", `${VALID}\n[project]\nname = "\u3000a"\n`],
    [ "edge-prompt-multiline-tab", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "a\\tb\\n\\u200cc"\n\n[roles.reviewer]')}`, ],
    [ "edge-prompt-cr", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "a\\rb"\n\n[roles.reviewer]')}`, ],
    [ "edge-bearer", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "use Bearer abcdefghijklmnop1234"\n\n[roles.reviewer]')}`, ],
    [ "edge-bearer-no-digit", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "use Bearer abcdefghijklmnopqrstu"\n\n[roles.reviewer]')}`, ],
    [ "edge-bearer-word", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "xBearer abcdefghijklmnop1234"\n\n[roles.reviewer]')}`, ],
    [ "edge-private-key", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "-----BEGIN RSA PRIVATE KEY-----"\n\n[roles.reviewer]')}`, ],
    [ "edge-private-key-lower", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "-----BEGIN rsa PRIVATE KEY-----"\n\n[roles.reviewer]')}`, ],
    [ "edge-aws-key", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "AKIAABCDEFGH"\n\n[roles.reviewer]')}`, ],
    [ "edge-github-token", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "ghp_abcdefgh12"\n\n[roles.reviewer]')}`, ],
    [ "edge-sk-token-short", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "sk-abcdefg"\n\n[roles.reviewer]')}`, ],
    [ "edge-sk-token-after-word", `${VALID.replace('host = "claude"\n\n[roles.reviewer]', 'host = "claude"\nprompt = "task-abcdefghij"\n\n[roles.reviewer]')}`, ],
  ];
  return cases.map(([name, content]) => ({
    name,
    entries: [
      typeof content === "string"
        ? file(content)
        : {
            path: "capstan.toml",
            base64: content.toString("base64"),
            mode: 0o600,
          },
    ],
  }));
}

export function configCases(): CorpusCase[] {
  const captured = readJson<CorpusCase[]>(CONFIG_CORPUS);
  const extra = edgeCases();
  const names = new Set<string>();
  for (const entry of [...captured, ...extra]) {
    if (names.has(entry.name)) throw new Error(`duplicate case ${entry.name}`);
    names.add(entry.name);
  }
  return [...captured, ...extra];
}

/** What a prompt case holds: the input as JSON, and the SHA-256 and byte length of the text (or the refusal). */
export interface PromptCase {
  readonly name: string;
  readonly input: PromptInput;
  /** A file of test/fixtures/prompts-off the text must equal. */
  readonly fixture?: string;
}

const WORKER_ROLES = [
  { name: "developer", kind: "Developer" },
  { name: "reviewer", kind: "Verifier" },
];
const ARCHITECT = {
  role: "architect",
  highRiskTriggers: ["schema or migrations", "security or auth"],
};
const OPERATOR = { role: "operator", autoApprove: ["ls -l"] };
const RESEARCHER = {
  role: "researcher",
  outputDir: "docs/research",
  userAgent: "capstan-researcher/1.0 (research bot; contact: project owner)",
};

function golden(kind: PromptInput["kind"]): PromptInput {
  return {
    roleName: "x",
    kind,
    agentId: "a-1",
    waitTimeoutSeconds: 90,
    rolePrompt: "Be brief.",
    workerRoles: WORKER_ROLES,
  };
}

/** The six prompts of test/fixtures/prompts-off. */
function goldenCases(): PromptCase[] {
  return [
    {
      name: "golden-pm",
      fixture: "pm",
      input: {
        ...golden("PM"),
        agentId: "pm-1",
        architect: ARCHITECT,
        operator: OPERATOR,
      },
    },
    {
      name: "golden-developer",
      fixture: "developer",
      input: {
        ...golden("Developer"),
        agentId: "developer-1",
        architect: ARCHITECT,
      },
    },
    {
      name: "golden-architect",
      fixture: "architect",
      input: {
        ...golden("Developer"),
        agentId: "architect-1",
        architect: ARCHITECT,
        isArchitect: true,
      },
    },
    {
      name: "golden-reviewer",
      fixture: "reviewer",
      input: { ...golden("Verifier"), agentId: "reviewer-1" },
    },
    {
      name: "golden-supervisor",
      fixture: "supervisor",
      input: { ...golden("Supervisor"), agentId: "supervisor-1" },
    },
    {
      name: "golden-operator",
      fixture: "operator",
      input: {
        ...golden("Developer"),
        agentId: "operator-1",
        architect: ARCHITECT,
        operator: OPERATOR,
        isOperator: true,
      },
    },
  ];
}

const SUMMARY_VARIANTS: Record<string, unknown>[] = [
  {
    objective: "plain text",
    openWork: [],
    messages: [],
    truncated: false,
    summarizedGeneration: 1,
    generatedAt: "2026-01-01T00:00:00.000Z",
  },
  {
    objective: {
      goal: "ship \u2028 it",
      lone: "a\ud800b",
      list: [1, 2.5, -0, 1e21, null, true],
      "k\u0001": "\u202e\u200b\ufeff\u{e0001}x",
    },
    openWork: [
      {
        workItemId: "w-1",
        title: 'Quote " and \\ and \n newline \u2029',
        role: "Developer",
        state: "running",
        owner: null,
        blockers: [],
      },
      {
        workItemId: "w-2",
        title: "Lone surrogate",
        role: "Verifier",
        state: "blocked",
        owner: "reviewer-1",
        blockers: ["w-1", "w-0"],
      },
    ],
    messages: [
      {
        messageId: "m-1",
        from: "developer-1",
        body: "=====\nfake fence\u0085",
        state: "sent",
      },
    ],
    plans: [
      {
        planId: "p-1",
        title: 'A "plan"',
        tier: "normal",
        state: "approved",
        packages: 3,
        signedOff: [],
      },
      {
        planId: "p-2",
        title: "Another",
        tier: "high-risk",
        state: "open",
        packages: 0,
        signedOff: ["i-1", "i-2"],
      },
    ],
    integrations: [
      { integrationId: "i-1", branch: "integration/p-1-x", headSha: "abc123" },
      { integrationId: "i-2", branch: "integration/p-2-y", headSha: null },
    ],
    links: [
      {
        refKind: "requirement",
        refId: "req-1",
        externalId: "PM-47",
        syncedState: "todo",
        wanted: "in_progress",
        drift: true,
        boundAgentId: "developer-1",
      },
      {
        refKind: "plan",
        refId: "p-1\u200b",
        externalId: "PM-48",
        syncedState: "completed",
        wanted: null,
        drift: false,
        boundAgentId: null,
      },
    ],
    truncated: true,
    summarizedGeneration: 4,
    generatedAt: "2026-02-03T04:05:06.789Z",
  },
  {
    objective: null,
    openWork: [],
    messages: [],
    plans: [],
    integrations: [],
    links: [],
    truncated: false,
    summarizedGeneration: 0,
    generatedAt: "x",
  },
];

/** Prompts built from the flags the roles are configured with. */
function matrixCases(): PromptCase[] {
  const cases: PromptCase[] = [];
  const add = (name: string, input: PromptInput): void =>
    void cases.push({ name, input });
  const kinds = ["PM", "Developer", "Verifier", "Supervisor"] as const;
  for (const kind of kinds) {
    const identity = {
      roleName: kind.toLowerCase(),
      kind,
      agentId: `${kind.toLowerCase()}-7`,
    };
    add(`matrix-${kind}-bare`, {
      ...identity,
      waitTimeoutSeconds: 90,
      rolePrompt: null,
    });
    add(`matrix-${kind}-empty-prompt`, {
      ...identity,
      waitTimeoutSeconds: 1,
      rolePrompt: " \n\t ",
    });
    add(`matrix-${kind}-trimmed-prompt`, {
      ...identity,
      waitTimeoutSeconds: 0.5,
      rolePrompt: "\u00a0\ufeff  Be brief.\n\n  ",
      workerRoles: [],
    });
    add(`matrix-${kind}-relay`, {
      ...identity,
      waitTimeoutSeconds: 3600,
      rolePrompt: null,
      promptRelay: { enabled: true },
    });
    add(`matrix-${kind}-seed`, {
      ...identity,
      waitTimeoutSeconds: 90,
      rolePrompt: "x",
      replacementSeed: "replacement seed {{0}} ${x} \\n",
    });
    for (const track of ["ask", "always", "never"] as const)
      for (const defaultAction of ["create", "link", "none"] as const) {
        const nexora = { track, defaultAction };
        add(`matrix-${kind}-nexora-${track}-${defaultAction}`, {
          ...identity,
          waitTimeoutSeconds: 90,
          rolePrompt: null,
          nexora,
        });
        if (kind === "PM") {
          add(`matrix-${kind}-nexora-${track}-${defaultAction}-plans`, {
            ...identity,
            waitTimeoutSeconds: 90,
            rolePrompt: null,
            nexora,
            architect: ARCHITECT,
          });
          add(`matrix-${kind}-nexora-${track}-${defaultAction}-all`, {
            ...identity,
            waitTimeoutSeconds: 90,
            rolePrompt: "Role.",
            nexora,
            architect: { role: "arch", highRiskTriggers: [] },
            operator: OPERATOR,
            researcher: RESEARCHER,
            promptRelay: { enabled: true },
            workerRoles: WORKER_ROLES,
          });
        }
      }
  }
  add("matrix-pm-architect-no-triggers", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    architect: { role: "a", highRiskTriggers: [] },
  });
  add("matrix-pm-researcher", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    researcher: RESEARCHER,
  });
  add("matrix-pm-operator", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    operator: OPERATOR,
  });
  add("matrix-pm-roles", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    workerRoles: WORKER_ROLES,
  });
  add("matrix-developer-researcher", {
    roleName: "researcher",
    kind: "Developer",
    agentId: "researcher-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    isResearcher: true,
    researcher: { ...RESEARCHER, outputDir: "a/b", userAgent: "ua $& {{0}}" },
  });
  add("matrix-developer-researcher-flag-only", {
    roleName: "researcher",
    kind: "Developer",
    agentId: "researcher-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    isResearcher: true,
  });
  add("matrix-developer-operator-flag-only", {
    roleName: "o",
    kind: "Developer",
    agentId: "o-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    isOperator: true,
  });
  add("matrix-developer-architect-flag-only", {
    roleName: "o",
    kind: "Developer",
    agentId: "o-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    isArchitect: true,
  });
  add("matrix-developer-architect-note", {
    roleName: "d",
    kind: "Developer",
    agentId: "d-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    architect: ARCHITECT,
  });
  add("matrix-developer-operator-and-researcher", {
    roleName: "d",
    kind: "Developer",
    agentId: "d-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    isOperator: true,
    operator: OPERATOR,
    isResearcher: true,
    researcher: RESEARCHER,
    isArchitect: true,
    architect: ARCHITECT,
  });
  add("matrix-verifier-researcher-ignored", {
    roleName: "v",
    kind: "Verifier",
    agentId: "v-1",
    waitTimeoutSeconds: 90,
    rolePrompt: null,
    isResearcher: true,
    researcher: RESEARCHER,
  });
  add("matrix-wait-large", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 1e21,
    rolePrompt: null,
  });
  add("matrix-wait-fraction", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 12.375,
    rolePrompt: null,
  });
  add("matrix-wait-tiny", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 1e-7,
    rolePrompt: null,
  });
  add("matrix-unicode", {
    roleName: "ünï",
    kind: "Developer",
    agentId: "😀-1",
    waitTimeoutSeconds: 90,
    rolePrompt: "日本語 \u2028 text",
  });
  SUMMARY_VARIANTS.forEach((restartSummary, index) => {
    add(`matrix-summary-${index}`, {
      roleName: "pm",
      kind: "PM",
      agentId: "pm-1",
      waitTimeoutSeconds: 90,
      rolePrompt: "Role.",
      restartSummary: restartSummary as unknown as NonNullable<
        PromptInput["restartSummary"]
      >,
      nexora: { track: "ask", defaultAction: "create" },
      architect: ARCHITECT,
    });
  });
  add("matrix-too-large", {
    roleName: "pm",
    kind: "PM",
    agentId: "pm-1",
    waitTimeoutSeconds: 90,
    rolePrompt: "x".repeat(160 * 1024),
  });
  add("matrix-just-fits", {
    roleName: "pm",
    kind: "Verifier",
    agentId: "pm-1",
    waitTimeoutSeconds: 90,
    rolePrompt: "x".repeat(160 * 1024 - 14000),
  });
  return cases;
}

export function promptCases(): PromptCase[] {
  const captured =
    readJson<{ name: string; input: PromptInput }[]>(PROMPT_CORPUS);
  const all = [...goldenCases(), ...captured, ...matrixCases()];
  const names = new Set<string>();
  for (const entry of all) {
    if (names.has(entry.name)) throw new Error(`duplicate case ${entry.name}`);
    names.add(entry.name);
  }
  return all;
}

type PromptExpected =
  | {
      readonly name: string;
      readonly fixture?: string;
      readonly bytes: number;
      readonly sha256: string;
    }
  | { readonly name: string; readonly error: string };

function expectedPrompt(source: PromptCase): PromptExpected {
  try {
    const text = buildRolePrompt(source.input);
    return {
      name: source.name,
      ...(source.fixture === undefined ? {} : { fixture: source.fixture }),
      bytes: Buffer.byteLength(text, "utf8"),
      sha256: createHash("sha256").update(text, "utf8").digest("hex"),
    };
  } catch (error) {
    return {
      name: source.name,
      error: error instanceof Error ? error.constructor.name : String(error),
    };
  }
}

/** Every committed expectation by file name, as the text the exporter writes. */
export function exportFixtures(): Map<string, string> {
  const configs = configCases().map((source) => ({
    name: source.name,
    outcome: shrink(evaluate(source.entries)),
  }));
  const prompts = promptCases().map(expectedPrompt);
  const text = (value: unknown): string =>
    `${JSON.stringify(value, null, 1)}\n`;
  const generated = createHash("sha256")
    .update(
      JSON.stringify(differentialTexts(DIFFERENTIAL_COUNT, DIFFERENTIAL_SEED)),
    )
    .digest("hex");
  return new Map([
    [
      DIFFERENTIAL,
      text({
        seed: DIFFERENTIAL_SEED,
        count: DIFFERENTIAL_COUNT,
        sha256: generated,
      }),
    ],
    [EDGE_CASES, text(edgeCases())],
    [PROMPT_BUILT, text([...goldenCases(), ...matrixCases()])],
    [CONFIG_EXPECTED, text(configs)],
    [PROMPT_EXPECTED, text(prompts)],
  ]);
}

// ---------------------------------------------------------------------------------------------------------------
// The differential run: mutated configurations, classified by smol-toml and, where it parses, by the whole loader.

/** mulberry32: a small seeded generator; the same seed gives the same cases on every machine. */
export function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const TOKEN =
  /"(?:[^"\\\n]|\\.)*"|'[^'\n]*'|[A-Za-z_][A-Za-z0-9_-]*|-?\d[\d_.:eE+-]*|\s+|./gu;
// prettier-ignore
const PIECES = [
  '"', "'", '"""', "'''", "[", "]", "[[", "]]", "{", "}", ",", "=", ".", "#", "\n", "\r\n", "\r", " ", "\t",
  "\\", "\\n", "\\u00e9", "\\U0001F600", "\\xE9", "\\e", "\\q", "\\uD800", "\\U00110000", "\\\n", "0", "1",
  "-1", "+1", "01", "1_000", "1__0", "_1", "1_", "0x1F", "0o7", "0b1", "0x", "1e3", "1.5", "1.", ".5", "inf",
  "-inf", "nan", "+nan", "99999999999999999999", "9223372036854775807", "9223372036854775808",
  "-9223372036854775809", "1979-05-27", "1979-05-27T07:32:00Z", "1979-05-27 07:32:00", "1979-05-27T07:32",
  "07:32:00", "07:32", "1979-13-45", "2024-02-30", "1979-05-27T07:32:00+25:00", "1979-05-27t07:32:00z",
  "true", "false", "True", "tru", "a.b", "a . b", '"a"."b"', "\u00e9", "\u{1F600}", "\u202e", "\u200b",
  "\ufeff", "\u0000", "\u007f", "\u0085", "\u2028", "__proto__", "constructor", "toString", '""', "''",
];

// prettier-ignore
const KEYS = [
  "a", "limits", "max_workers", "kind", "host", "name", "schema_version", "x-y", "x_y", "1", "01", "a.b",
  "a.b.c", '"a b"', "'q'", '"é"', "ü", '""', "true", "inf", "nan", "0x1", "1e3", "__proto__", "roles.pm",
  "hosts.claude.kind", "env.pass",
];

// prettier-ignore
const SCALARS = [
  "0", "1", "-1", "+1", "007", "1_0", "0x10", "0o10", "0b10", "0xdead_beef", "1.0", "-0.0", "1e10", "1E+3",
  "6.02e23", "inf", "-inf", "+nan", "9223372036854775807", "-9223372036854775808", "9223372036854775808",
  "340282366920938463463374607431768211455", "true", "false", '""', "''", '"x"', "'x'", '"""a"""', "'''a'''",
  '"\\u00e9"', '"\\U0001F600"', '"\\x41"', '"\\e"', '"\\q"', '"\\"', "1979-05-27", "1979-05-27T07:32:00",
  "1979-05-27T07:32:00Z", "1979-05-27 07:32:00", "1979-05-27T07:32:00.999999999-07:00", "07:32:00", "07:32",
  "00:00:60", "2024-02-29", "2023-02-29", "0000-00-00", "9999-12-31", "[]", "[1]", '["a"]', "[1, 2,]", "[,]",
  "[[1], [2]]", '[1, "a"]', "{}", "{a=1}", "{a=1,}", "{a=1,\nb=2}", "{ a = { b = 1 } }", "{a.b=1}",
  "{a=1, a=2}",
];

function randomValue(next: () => number, depth = 0): string {
  const roll = next();
  if (depth < 2 && roll < 0.15)
    return `[${Array.from({ length: Math.floor(next() * 3) }, () => randomValue(next, depth + 1)).join(next() < 0.2 ? ",\n" : ", ")}${next() < 0.2 ? "," : ""}]`;
  if (depth < 2 && roll < 0.3)
    return `{${Array.from({ length: Math.floor(next() * 3) }, () => `${KEYS[Math.floor(next() * KEYS.length)]} = ${randomValue(next, depth + 1)}`).join(next() < 0.2 ? ",\n" : ", ")}${next() < 0.15 ? "," : ""}}`;
  return SCALARS[Math.floor(next() * SCALARS.length)] ?? "0";
}

function tokens(text: string): string[] {
  return text.match(TOKEN) ?? [];
}

/** One random damage to `text`. */
function mutateOnce(text: string, next: () => number): string {
  const parts = tokens(text);
  const pick = (count: number): number => Math.floor(next() * count);
  if (parts.length === 0) return text + PIECES[pick(PIECES.length)];
  const at = pick(parts.length);
  const piece = PIECES[pick(PIECES.length)] ?? "";
  switch (pick(16)) {
    case 12: {
      // a whole line `key = value` lands between two lines
      const lines = text.split("\n");
      const line = `${KEYS[pick(KEYS.length)]} = ${randomValue(next)}`;
      lines.splice(
        pick(lines.length + 1),
        0,
        next() < 0.15 ? `[${KEYS[pick(KEYS.length)]}]` : line,
      );
      return lines.join("\n");
    }
    case 13: {
      // the value of one line becomes another value
      const lines = text.split("\n");
      const candidates = lines
        .map((l, i) => [l, i] as const)
        .filter(([l]) => /^\s*[^#=\n]+=/.test(l));
      const hit = candidates[pick(Math.max(1, candidates.length))];
      if (hit === undefined) return text + piece;
      lines[hit[1]] =
        `${hit[0].slice(0, hit[0].indexOf("=") + 1)} ${randomValue(next)}`;
      return lines.join("\n");
    }
    case 14: {
      // a table header or key is rewritten (dotted, quoted, repeated, unicode)
      const lines = text.split("\n");
      const hit = pick(lines.length);
      lines[hit] =
        next() < 0.5
          ? `[${KEYS[pick(KEYS.length)]}]`
          : `${KEYS[pick(KEYS.length)]} = ${randomValue(next)}`;
      return lines.join("\n");
    }
    case 15: {
      // line endings and whitespace
      const endings = [
        "\r\n",
        "\r",
        "\n\n",
        " \n",
        "\t\n",
        "\u000b\n",
        "\u00a0\n",
      ];
      return text.split("\n").join(endings[pick(endings.length)] ?? "\n");
    }
    case 0:
      parts.splice(at, 1);
      break;
    case 1:
      parts.splice(at, 0, parts[at] ?? "");
      break;
    case 2:
      parts.splice(at, 0, piece);
      break;
    case 3:
      parts[at] = piece;
      break;
    case 4: {
      const swap = pick(parts.length);
      [parts[at], parts[swap]] = [parts[swap] ?? "", parts[at] ?? ""];
      break;
    }
    case 5:
      return text.slice(0, pick(text.length + 1));
    case 6: {
      const cut = pick(text.length + 1);
      return text.slice(0, cut) + piece + text.slice(cut);
    }
    case 7: {
      // quote damage
      const quotes = [...text.matchAll(/["']/g)];
      const hit = quotes[pick(Math.max(1, quotes.length))];
      if (hit?.index === undefined) return text + piece;
      return text.slice(0, hit.index) + text.slice(hit.index + 1);
    }
    case 8: {
      // bracket damage
      const brackets = [...text.matchAll(/[[\]{}]/g)];
      const hit = brackets[pick(Math.max(1, brackets.length))];
      if (hit?.index === undefined) return text + piece;
      return (
        text.slice(0, hit.index) +
        (next() < 0.5 ? "" : piece) +
        text.slice(hit.index + 1)
      );
    }
    case 9: {
      // a number or date token becomes another number or date
      const numeric = parts
        .map((p, i) => [p, i] as const)
        .filter(([p]) => /^[-+]?\d/.test(p));
      const hit = numeric[pick(Math.max(1, numeric.length))];
      if (hit === undefined) return text + piece;
      parts[hit[1]] = piece;
      break;
    }
    case 10: {
      // a key becomes another key (duplicates, dotted, quoted, unicode)
      const words = parts
        .map((p, i) => [p, i] as const)
        .filter(([p]) => /^[A-Za-z_]/.test(p));
      const hit = words[pick(Math.max(1, words.length))];
      if (hit === undefined) return text + piece;
      parts[hit[1]] =
        next() < 0.5 ? (words[pick(words.length)]?.[0] ?? piece) : piece;
      break;
    }
    default: {
      // an escape lands inside a string
      const strings = parts
        .map((p, i) => [p, i] as const)
        .filter(([p]) => p.startsWith('"'));
      const hit = strings[pick(Math.max(1, strings.length))];
      if (hit === undefined) return text + piece;
      parts[hit[1]] = hit[0].slice(0, -1) + piece + '"';
      break;
    }
  }
  return parts.join("");
}

export function mutate(text: string, next: () => number): string {
  let out = text;
  const rounds = 1 + Math.floor(next() * 3);
  for (let round = 0; round < rounds; round += 1) out = mutateOnce(out, next);
  return out;
}

export interface DifferentialCase {
  readonly index: number;
  /** The mutated file, base64. */
  readonly base64: string;
  /** What smol-toml makes of it. */
  readonly toml: "ok" | "error";
  /** What the whole loader makes of it in an empty project, when the TOML parses; `error` carries the message. */
  readonly loader?:
    | { readonly kind: "ok"; readonly json: string }
    | { readonly kind: "error"; readonly message: string };
}

/** The classification of one mutated text, as the differential run records it. */
export function classify(
  index: number,
  text: string,
  project: string,
): DifferentialCase {
  const bytes = Buffer.from(text, "utf8");
  const base64 = bytes.toString("base64");
  try {
    const config = parseCapstanConfig(bytes, project);
    return {
      index,
      base64,
      toml: "ok",
      loader: { kind: "ok", json: JSON.stringify(config, null, 2) },
    };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (TOML_ERROR.test(message)) return { index, base64, toml: "error" };
    return { index, base64, toml: "ok", loader: { kind: "error", message } };
  }
}

/** `count` mutated configurations of the corpus, from `seed`. Text that is not valid UTF-8 never reaches the parser, so only text corpus files are mutated. */
export function differentialTexts(count: number, seed: number): string[] {
  const next = random(seed);
  const sources = [
    ...configCases()
      .map(
        (source) =>
          source.entries.find((entry) => entry.path === "capstan.toml")?.text,
      )
      .filter(
        (text): text is string =>
          text !== undefined && text.length > 0 && text.length < 20000,
      ),
  ];
  const texts: string[] = [];
  for (let index = 0; index < count; index += 1) {
    const source = sources[Math.floor(next() * sources.length)] ?? VALID;
    texts.push(mutate(source, next));
  }
  return texts;
}

// ---------------------------------------------------------------------------------------------------------------
// How the corpora were captured: the existing tests run with the loaders wrapped to record what they were given.

/** Hooks that wrap `loadCapstanConfig` (`configs`) or `buildRolePrompt` (`prompts`) of the built code and append what they see to $CAPTURE_FILE. */
function captureHooks(which: "configs" | "prompts"): string {
  const target =
    which === "configs"
      ? "/dist/src/config/capstan-config.js"
      : "/dist/src/prompts.js";
  const wrapper =
    which === "configs"
      ? `
import __fs from "node:fs"; import __path from "node:path";
function __walk(root, rel, out, depth) {
  if (depth > 5) return;
  let entries; try { entries = __fs.readdirSync(__path.join(root, rel), { withFileTypes: true }); } catch { return; }
  for (const e of entries) {
    if (e.name === ".git") continue;
    const r = rel ? rel + "/" + e.name : e.name; const full = __path.join(root, r); const st = __fs.lstatSync(full);
    if (e.isSymbolicLink()) out.push({ path: r, link: __fs.readlinkSync(full).split(root).join("$ROOT") });
    else if (e.isDirectory()) { out.push({ path: r, dir: true }); __walk(root, r, out, depth + 1); }
    else if (e.isFile() && st.size < 200000) out.push({ path: r, base64: __fs.readFileSync(full).toString("base64"), mode: st.mode & 0o7777 });
  }
}
export function loadCapstanConfig(root) {
  const entries = []; __walk(root, "", entries, 0);
  __fs.appendFileSync(process.env.CAPTURE_FILE, JSON.stringify({ entries }) + "\\n");
  return __origLoad(root);
}`
      : `
import __fs from "node:fs";
export function buildRolePrompt(input) {
  __fs.appendFileSync(process.env.CAPTURE_FILE, JSON.stringify({ input }) + "\\n");
  return __origBuild(input);
}`;
  const original =
    which === "configs" ? "loadCapstanConfig" : "buildRolePrompt";
  const renamed = which === "configs" ? "__origLoad" : "__origBuild";
  return `export async function load(url, context, nextLoad) {
  const result = await nextLoad(url, context);
  if (!url.endsWith(${JSON.stringify(target)})) return result;
  const source = result.source.toString().replace("export function ${original}(", "function ${renamed}(") + ${JSON.stringify(wrapper)};
  return { ...result, source };
}`;
}

/** Folds a capture file into a corpus: duplicates dropped, entries kept as recorded. */
export function foldCapture(
  which: "configs" | "prompts",
  captureFile: string,
): string {
  const lines = readFileSync(captureFile, "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line) as Record<string, unknown>);
  const seen = new Set<string>();
  const kept: unknown[] = [];
  for (const line of lines) {
    const key = JSON.stringify(which === "configs" ? line.entries : line.input);
    if (seen.has(key)) continue;
    if (which === "prompts" && key.length > 60000) continue;
    if (which === "configs" && key.length > 30000) continue;
    seen.add(key);
    const name = `${which === "configs" ? "captured" : "captured-prompt"}-${String(kept.length + 1).padStart(4, "0")}`;
    kept.push(
      which === "configs"
        ? { name, entries: textual(line.entries as CorpusEntry[]) }
        : { name, input: line.input },
    );
  }
  return `${JSON.stringify(kept, null, 1)}\n`;
}

/** Files that are valid text without control characters are kept as text, so the corpus reads and diffs. */
function textual(entries: CorpusEntry[]): CorpusEntry[] {
  return entries.map((entry) => {
    if (entry.base64 === undefined) return entry;
    const whole = Buffer.from(entry.base64, "base64").toString("utf8");
    const mode = entry.mode === undefined ? {} : { mode: entry.mode };
    if (whole === STARTER_CONFIG)
      return { path: entry.path, ref: "starter", ...mode };
    if (whole === DESIGNER_PROMPT)
      return { path: entry.path, ref: "designer", ...mode };
    const bytes = Buffer.from(entry.base64, "base64");
    const text = bytes.toString("utf8");
    if (
      Buffer.from(text, "utf8").equals(bytes) &&
      !/[\u0000-\u0008\u000b-\u001f\u007f]/.test(text) &&
      !text.includes("\r")
    )
      return {
        path: entry.path,
        text,
        ...(entry.mode === undefined ? {} : { mode: entry.mode }),
      };
    return entry;
  });
}

/** A scenario of cli-transcript-export.ts: `cstan config check` in a project directory, run by the Node CLI and replayed on the front end. */
export interface TranscriptCase {
  readonly name: string;
  readonly argv: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly dirs?: readonly string[];
  readonly cwd: string;
  readonly contents?: Readonly<Record<string, string>>;
  readonly modes?: Readonly<Record<string, number>>;
  readonly links?: readonly { readonly path: string; readonly to: string }[];
  readonly fallback?: boolean;
  readonly skipNode?: boolean;
}

const CONFIG_VALID = `schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"
`;

/** `cstan config check` in a project directory holding a capstan.toml, answered by the front end or handed to Node. */
function configCase(
  name: string,
  toml: string | null,
  extra: Partial<TranscriptCase> = {},
  argv: readonly string[] = ["config", "check"],
): TranscriptCase {
  return {
    name: `config-check-${name}`,
    argv,
    env: { TZ: "UTC" },
    dirs: ["proj"],
    cwd: "proj",
    ...extra,
    contents: {
      ...(toml === null ? {} : { "proj/capstan.toml": toml }),
      ...extra.contents,
    },
  };
}

export function configCheckCases(): TranscriptCase[] {
  const withTable = (table: string): string => `${CONFIG_VALID}\n${table}`;
  const handedToNode = (
    name: string,
    toml: string | null,
    extra: Partial<TranscriptCase> = {},
    argv?: readonly string[],
  ): TranscriptCase => ({
    ...configCase(name, toml, extra, argv),
    fallback: true,
  });
  const codex = `schema_version = 1

[hosts.claude]
kind = "claude"

[hosts.codex]
kind = "codex"

[roles.pm]
kind = "PM"
host = "claude"

[roles.worker]
kind = "Developer"
host = "codex"
permission_mode = "acceptEdits"
`;
  const mcp = `${CONFIG_VALID}
[mcp_servers.docs]
command = "npx"
args = ["-y", "some-docs-server"]

[mcp_servers.latest]
command = "npx"
args = ["-y", "other-server@latest"]

[mcp_servers.pinned]
command = "/usr/local/bin/pinned"
args = ["--flag"]

[mcp_servers.versioned]
command = "bunx"
args = ["pkg@1.2.3"]
`;
  const officers = `schema_version = 1

[hosts.claude]
kind = "claude"

[architect]
enabled = true
reviewer_role = "reviewer"

[operator]
enabled = true
auto_approve = ["ls -l", "git rev-parse --short HEAD"]
auto_approve_prefix = ["ls"]

[researcher]
enabled = true

[prompt_relay]
enabled = true

[nexora]
track = "never"

[roles.pm]
kind = "PM"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"

[roles.architect]
kind = "Developer"
host = "claude"

[roles.operator]
kind = "Developer"
host = "claude"
allow = ["Bash(cstan op *)"]
deny = ["Write", "Edit", "NotebookEdit", "Agent", "Task", "Read", "Glob", "Grep"]

[roles.researcher]
kind = "Developer"
host = "claude"
allow = ["WebSearch", "WebFetch", "Write(docs/research/**)"]
deny = ["Agent", "Task", "NotebookEdit", "Bash(git push)", "Bash(git push *)", "Bash(git merge *)", "Bash(git rebase *)", "Bash(git reset *)", "Bash(git remote *)", "Bash(git config *)", "Bash(git checkout *)", "Bash(git switch *)", "Bash(curl * -d*)", "Bash(curl * --data*)", "Bash(curl * -F*)", "Bash(curl * --form*)", "Bash(curl * -T*)", "Bash(curl * --upload-file*)", "Bash(curl * -X*)", "Bash(curl * --request*)", "Bash(curl * --json*)", "Bash(curl * -o*)", "Bash(curl * --output*)", "Bash(curl * -O*)", "Bash(curl * --remote-name*)", "Bash(curl * -K*)", "Bash(curl * --config*)", "Bash(curl * -u*)", "Bash(curl * --user*)", "Bash(curl * -c*)", "Bash(curl * --cookie-jar*)", "Bash(curl * -D*)", "Bash(curl * --dump-header*)", "Bash(curl * --trace*)", "Bash(curl * --stderr*)", "Bash(curl * --create-dirs*)", "Bash(curl * --libcurl*)", "Bash(curl * --hsts*)", "Bash(curl * --alt-svc*)", "Bash(curl * --etag-save*)", "Bash(curl * file:*)", "Bash(curl * @*)", "Bash(curl -d*)", "Bash(curl --data*)", "Bash(curl -F*)", "Bash(curl --form*)", "Bash(curl -T*)", "Bash(curl --upload-file*)", "Bash(curl -X*)", "Bash(curl --request*)", "Bash(curl --json*)", "Bash(curl -o*)", "Bash(curl --output*)", "Bash(curl -O*)", "Bash(curl --remote-name*)", "Bash(curl -K*)", "Bash(curl --config*)", "Bash(curl -u*)", "Bash(curl --user*)", "Bash(curl -c*)", "Bash(curl --cookie-jar*)", "Bash(curl -D*)", "Bash(curl --dump-header*)", "Bash(curl --trace*)", "Bash(curl --stderr*)", "Bash(curl --create-dirs*)", "Bash(curl --libcurl*)", "Bash(curl --hsts*)", "Bash(curl --alt-svc*)", "Bash(curl --etag-save*)", "Bash(curl *%output{*)", "Bash(curl file:*)", "Bash(curl @*)", "Bash(git * --output*)"]
`;
  return [
    // answered natively
    configCase("valid", CONFIG_VALID),
    configCase("starter", STARTER_CONFIG, {
      contents: { "proj/roles/designer.md": DESIGNER_PROMPT },
    }),
    configCase("bom", `﻿${CONFIG_VALID}`),
    configCase("crlf", CONFIG_VALID.replace(/\n/g, "\r\n")),
    configCase("never-tracked", withTable('[nexora]\ntrack = "never"\n')),
    configCase(
      "layout-split-warning",
      withTable('[layout]\nsplit = "right"\n'),
    ),
    configCase("full-access-host-warning", codex),
    configCase("mcp-warnings", mcp),
    configCase("architect-operator-researcher", officers),
    configCase(
      "project-name-unicode",
      withTable('[project]\nname = "プロジェクト 😀"\n'),
    ),
    configCase(
      "dotted-keys",
      `schema_version = 1\nhosts.claude.kind = "claude"\nroles.pm.kind = "PM"\nroles.pm.host = "claude"\n`,
    ),
    configCase(
      "prompt-inline-and-file",
      `${CONFIG_VALID.replace('[roles.developer]\nkind = "Developer"\nhost = "claude"', '[roles.developer]\nkind = "Developer"\nhost = "claude"\nprompt_file = "prompts/dev.md"')}\n[roles.writer]\nkind = "Developer"\nhost = "claude"\nprompt = "Write small commits."\n`,
      {
        contents: { "proj/prompts/dev.md": "Be brief.\r\nAnd kind.\n" },
      },
    ),
    configCase(
      "numeric-role-order",
      `schema_version = 1\n[hosts.claude]\nkind = "claude"\n[roles.zeta]\nkind = "PM"\nhost = "claude"\n[roles.10]\nkind = "Developer"\nhost = "claude"\n`,
    ),
    configCase("unknown-key", withTable("[limits]\nmax_worker = 3\n")),
    configCase(
      "wrong-schema",
      CONFIG_VALID.replace("schema_version = 1", "schema_version = 2"),
    ),
    configCase(
      "schema-text",
      CONFIG_VALID.replace("schema_version = 1", 'schema_version = "1"'),
    ),
    configCase(
      "no-pm",
      CONFIG_VALID.replace('kind = "PM"', 'kind = "Developer"'),
    ),
    configCase(
      "two-pms",
      withTable('[roles.second]\nkind = "PM"\nhost = "claude"\n'),
    ),
    configCase(
      "role-without-host",
      withTable('[roles.ghost]\nkind = "Developer"\nhost = "nowhere"\n'),
    ),
    configCase(
      "bad-role-name",
      withTable('[roles.Bad_Name]\nkind = "Developer"\nhost = "claude"\n'),
    ),
    configCase(
      "credential-in-prompt",
      withTable(
        '[roles.leaky]\nkind = "Developer"\nhost = "claude"\nprompt = "use sk-abcdefghijkl"\n',
      ),
    ),
    configCase(
      "integer-out-of-range",
      withTable("[limits]\nmax_workers = 99\n"),
    ),
    configCase(
      "string-for-integer",
      withTable('[limits]\nmax_workers = "3"\n'),
    ),
    configCase(
      "prompt-file-missing",
      CONFIG_VALID.replace(
        'host = "claude"\n\n[roles.developer]',
        'host = "claude"\nprompt_file = "nope.md"\n\n[roles.developer]',
      ),
    ),
    configCase(
      "prompt-file-outside",
      CONFIG_VALID.replace(
        'host = "claude"\n\n[roles.developer]',
        'host = "claude"\nprompt_file = "../outside.md"\n\n[roles.developer]',
      ),
      {
        contents: { "outside.md": "outside\n" },
      },
    ),
    configCase(
      "operator-conflict",
      withTable(
        '[operator]\nenabled = true\nrole = "developer"\nauto_approve = ["rm -rf x"]\n',
      ),
    ),
    configCase("symlinked-file", null, {
      contents: { "proj/real.toml": CONFIG_VALID },
      links: [{ path: "proj/capstan.toml", to: "proj/real.toml" }],
    }),
    configCase("writable-by-group", CONFIG_VALID, {
      modes: { "proj/capstan.toml": 0o660 },
    }),
    configCase("directory-in-place", null, {
      dirs: ["proj", "proj/capstan.toml"],
    }),
    // handed to Node
    handedToNode("missing-file", null),
    handedToNode(
      "toml-unterminated-string",
      withTable('[project]\nname = "oops\n'),
    ),
    handedToNode("toml-bad-key", `${CONFIG_VALID}\n= 3\n`),
    handedToNode("toml-duplicate-key", `schema_version = 1\n${CONFIG_VALID}`),
    handedToNode(
      "toml-secret-line",
      `${CONFIG_VALID}\ntoken = sk-abcdefghijkl\n`,
    ),
    handedToNode("float", withTable("[limits]\nmax_workers = 3.5\n")),
    handedToNode(
      "datetime",
      withTable("[limits]\nmax_workers = 1979-05-27T07:32:00Z\n"),
    ),
    handedToNode(
      "big-integer",
      withTable("[limits]\nmax_workers = 99999999999999999999\n"),
    ),
    handedToNode("double-bom", `﻿﻿${CONFIG_VALID}`),
    handedToNode(
      "proto-key",
      withTable('[mcp_servers.__proto__]\ncommand = "x"\n'),
    ),
    handedToNode(
      "inline-table-newline",
      `schema_version = 1\nhosts = { claude = { kind = "claude" } }\nroles = {\n  pm = { kind = "PM", host = "claude" }\n}\n`,
    ),
    handedToNode("extra-argument", CONFIG_VALID, {}, [
      "config",
      "check",
      "now",
    ]),
    handedToNode("sync", CONFIG_VALID, { skipNode: true }, ["config", "sync"]),
    handedToNode("no-subcommand", CONFIG_VALID, {}, ["config"]),
  ];
}

if (import.meta.filename === process.argv[1]) {
  const [flag, a, b, c] = process.argv.slice(2);
  if (flag === "--differential") {
    const project = realpathSync(
      mkdtempSync(path.join(os.tmpdir(), "cstan-cfgdiff-")),
    );
    try {
      const lines = differentialTexts(Number(a), Number(b)).map((text, index) =>
        JSON.stringify(classify(index, text, project)),
      );
      writeFileSync(c ?? "differential.jsonl", `${lines.join("\n")}\n`);
      process.stdout.write(`wrote ${lines.length} cases to ${c}\n`);
    } finally {
      rmSync(project, { recursive: true, force: true });
    }
  } else if (flag === "--fold-configs" || flag === "--fold-prompts") {
    const which = flag === "--fold-configs" ? "configs" : "prompts";
    const out = path.join(
      PARITY_DIRECTORY,
      which === "configs" ? CONFIG_CORPUS : PROMPT_CORPUS,
    );
    writeFileSync(out, foldCapture(which, a ?? ""));
    process.stdout.write(`wrote ${out}\n`);
  } else if (flag === "--show-config" || flag === "--show-prompt") {
    const wanted = a;
    if (flag === "--show-config") {
      const found = configCases().find((source) => source.name === wanted);
      if (found === undefined) throw new Error(`no case ${wanted}`);
      const outcome = evaluate(found.entries);
      process.stdout.write(
        outcome.kind === "ok"
          ? outcome.json
          : `${outcome.kind}: ${outcome.message}\n`,
      );
    } else {
      const found = promptCases().find((source) => source.name === wanted);
      if (found === undefined) throw new Error(`no case ${wanted}`);
      process.stdout.write(buildRolePrompt(found.input));
    }
  } else if (flag === "--print-capture-hooks") {
    process.stdout.write(
      `${captureHooks(a === "prompts" ? "prompts" : "configs")}\n`,
    );
  } else {
    mkdirSync(PARITY_DIRECTORY, { recursive: true });
    for (const needed of [CONFIG_CORPUS, PROMPT_CORPUS])
      if (!existsSync(path.join(PARITY_DIRECTORY, needed)))
        throw new Error(
          `${needed} is missing; capture it first (see the header)`,
        );
    for (const [name, text] of exportFixtures())
      writeFileSync(path.join(PARITY_DIRECTORY, name), text);
    process.stdout.write(`wrote the expectations to ${PARITY_DIRECTORY}\n`);
  }
}

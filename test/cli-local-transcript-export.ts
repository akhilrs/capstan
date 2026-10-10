/**
 * Writes the transcripts of the commands the Rust `cstan` has to serve without Node: the ones that run on the local
 * machine (version, help, usage errors, init, config, herdr-config, start, stop, status and inspect over a ledger
 * file, dash refusals, the commands that reach a daemon as the operator). Each one runs the Node CLI
 * (dist/src/cli.js) on fixed argv, environment, directory layout and stdin and records what it printed, how it
 * exited, and the files it created. The format is that of rust/crates/cstan/tests/transcripts (argv, env, cwd,
 * layout, stdin, daemon, request, node.stdout, node.stderr, node.exit) plus `files`, the tree and bytes a command
 * creates, and a `daemon` of kind `scratch` for a run against a real Node daemon of the scratch project.
 *
 * Normalisation follows test/cli-parity-harness.ts: ids, times, pids and scratch paths become placeholders that keep
 * their kind (`<ID#2>`, `<TS:z>`, `<PID>`, `<ROOT>`, `<TOKEN:operator>`), the package version becomes `<VERSION>`;
 * everything else is exact. Inputs (argv, env, layout) name the scratch directory `$ROOT` and the host PATH `$PATH`.
 *
 * Run `npm run build && node dist/test/cli-local-transcript-export.js` after an intended change and commit the result;
 * cli-local-transcript.test.ts fails while the committed files differ from a fresh export. The ledgers under
 * local-transcripts/ledgers are inputs, rebuilt only by `node dist/test/cli-local-transcript-export.js --ledgers`.
 */
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { ControllerCore } from "../src/controller/core.js";
import { openSqlite } from "../src/controller/sqlite.js";
import { STARTER_CONFIG } from "../src/config/capstan-config.js";
import { DESIGNER_PROMPT } from "../src/roles/designer-prompt.js";
import { Normaliser } from "./cli-parity-harness.js";
import {
  PACKAGE_VERSION,
  normaliseVersion,
  type ExportOptions,
} from "./cli-transcript-export.js";
import { insertWorkItem } from "./legacy-rows.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const LOCAL_TRANSCRIPT_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "cstan",
  "tests",
  "local-transcripts",
);
const LEDGER_DIRECTORY = path.join(LOCAL_TRANSCRIPT_DIRECTORY, "ledgers");
const MIGRATION_DIRECTORY = path.join(root, "migrations");
const CLI = path.join(root, "dist", "src", "cli.js");

/** The fixed `Date.now()` of every run of a client (not of a daemon). */
export const NOW = Date.parse("2026-10-07T12:00:00.000Z");
/** The PATH a run gets for `$PATH`: git and the system tools, never the host's own bin directories. */
const HOST_PATH = "/usr/local/bin:/usr/bin:/bin";
const STEP_LIMIT_MS = 60_000;

/** What the fake daemon of a case sends once it has read the request line (the kinds of cli-transcript-export.ts). */
export type Reply =
  | { readonly kind: "line"; readonly text: string }
  | { readonly kind: "close" }
  | { readonly kind: "hang" };

export interface Step {
  readonly argv: readonly string[];
  /** The whole environment of the step; the environment of the case when absent. */
  readonly env?: Readonly<Record<string, string>>;
}

export interface Layout {
  readonly dirs: readonly string[];
  readonly files: readonly string[];
  /** Files with their text (`$ROOT` stands for the scratch directory). */
  readonly contents: Readonly<Record<string, string>>;
  /** Octal modes, as text; files default to 0600, directories to 0755. */
  readonly modes: Readonly<Record<string, string>>;
  /** Copies of committed files below local-transcripts, by destination. */
  readonly copy: Readonly<Record<string, string>>;
  readonly links: readonly { readonly path: string; readonly to: string }[];
  /** Directories that are git work trees: `git init`, then (commit) an initial commit of everything not ignored. */
  readonly git: readonly { readonly dir: string; readonly commit: boolean }[];
}

export interface FileEntry {
  readonly path: string;
  readonly kind: "dir" | "file";
  readonly mode: string;
  readonly text?: string;
}

export type Daemon =
  | { readonly kind: "fake"; readonly socket: string; readonly reply: Reply }
  | { readonly kind: "scratch" };

export interface LocalTranscript {
  readonly name: string;
  readonly argv: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly layout: Layout;
  readonly cwd: string;
  readonly stdin: string;
  /** Run on a pseudo-terminal: stdout and stderr arrive merged in `node.stdout`. */
  readonly tty: boolean;
  readonly now: number;
  /** Commands run first, by the Node CLI, to bring the scratch project to the state the case starts from. */
  readonly before: readonly Step[];
  readonly daemon: Daemon | null;
  /** The command is stopped once its stdout holds this text (a watch that never ends). */
  readonly stopAfter: string | null;
  readonly request: string | null;
  readonly node: {
    readonly stdout: string;
    readonly stderr: string;
    readonly exit: number | null;
    readonly signal: string | null;
  };
  /** The files the command created or changed, when the case records them. */
  readonly files: readonly FileEntry[] | null;
  /** The commits of each git work tree after the command, when the case records files. */
  readonly commits: Readonly<
    Record<string, readonly { subject: string; files: readonly string[] }[]>
  > | null;
}

interface Case {
  readonly name: string;
  readonly argv: readonly string[];
  readonly env?: Readonly<Record<string, string>>;
  readonly layout?: Partial<Layout>;
  readonly cwd?: string;
  readonly stdin?: string;
  readonly tty?: boolean;
  readonly before?: readonly Step[];
  readonly socket?: string;
  readonly reply?: Reply;
  /** A run against a real Node daemon of the scratch project. */
  readonly scratch?: boolean;
  readonly stopAfter?: string;
  readonly recordFiles?: boolean;
}

const SOCKET = "$ROOT/proj/.capstan/state/control.sock";
const TOKEN = "tok-0123456789abcdef0123456789abcdef";
const OPERATOR_KEY =
  "fixture-operator-key-0123456789abcdefghijklmnopqrstuvwxyz";
const PROJECT_ID = "pfixture";

const BASE_ENV: Readonly<Record<string, string>> = {
  TZ: "UTC",
  PATH: "$PATH",
  HOME: "$ROOT/home",
};
const GIT_ENV: Readonly<Record<string, string>> = {
  ...BASE_ENV,
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_AUTHOR_NAME: "Fixture",
  GIT_AUTHOR_EMAIL: "fixture@example.com",
  GIT_COMMITTER_NAME: "Fixture",
  GIT_COMMITTER_EMAIL: "fixture@example.com",
};
const OFF_ENV: Readonly<Record<string, string>> = {
  ...BASE_ENV,
  CAPSTAN_LAUNCH: "off",
};
const AGENT_ENV: Readonly<Record<string, string>> = {
  ...OFF_ENV,
  CAPSTAN_TOKEN: TOKEN,
  CAPSTAN_SOCKET: SOCKET,
  CAPSTAN_AGENT_ID: "developer-1",
};
const FIXTURE_LEDGERS = [
  { name: "v20", migrations: 20 },
  { name: "v30", migrations: 30 },
  { name: "v36", migrations: 36 },
  { name: "v37", migrations: 37 },
] as const;

function emptyLayout(partial: Partial<Layout> = {}): Layout {
  return {
    dirs: partial.dirs ?? [],
    files: partial.files ?? [],
    contents: partial.contents ?? {},
    modes: partial.modes ?? {},
    copy: partial.copy ?? {},
    links: partial.links ?? [],
    git: partial.git ?? [],
  };
}

interface ProjectOptions {
  /** A committed ledger copied to the state directory. */
  readonly ledger?: string;
  /** No capstan.toml and no designer prompt. */
  readonly bare?: boolean;
  /** The text of capstan.toml, instead of the starter. */
  readonly toml?: string;
  readonly git?: "commit" | "init" | "none";
}

/** An initialised project in `proj` with a fixed operator key and project id, as `cstan init` leaves one. */
function project(options: ProjectOptions = {}): Partial<Layout> {
  const contents: Record<string, string> = {
    "proj/.capstan/operator.key": `${OPERATOR_KEY}\n`,
    "proj/.capstan/project.json": `${JSON.stringify(
      {
        schemaVersion: 1,
        projectId: PROJECT_ID,
        name: "proj",
        stateDirectory: "$ROOT/proj/.capstan/state",
        maxSlices: 4,
        maxRunMs: 3_600_000,
        maxDispatches: 16,
      },
      null,
      2,
    )}\n`,
  };
  if (options.bare !== true) {
    contents["proj/capstan.toml"] = options.toml ?? STARTER_CONFIG;
    contents["proj/roles/designer.md"] = DESIGNER_PROMPT;
  }
  const git = options.git ?? "commit";
  return {
    dirs: ["proj/.capstan/state", "home"],
    contents,
    modes: {
      "proj/.capstan": "0700",
      "proj/.capstan/state": "0700",
      "proj/.capstan/operator.key": "0600",
      "proj/.capstan/project.json": "0600",
      "proj/roles/designer.md": "0644",
    },
    copy:
      options.ledger === undefined
        ? {}
        : {
            "proj/.capstan/state/controller.sqlite": `ledgers/${options.ledger}.sqlite3`,
          },
    git: git === "none" ? [] : [{ dir: "proj", commit: git === "commit" }],
  };
}

function operatorCase(
  name: string,
  argv: readonly string[],
  extra: Partial<Case> = {},
): Case {
  return {
    name,
    argv,
    env: OFF_ENV,
    cwd: "proj",
    layout: project(),
    scratch: true,
    ...extra,
  };
}

const CONTROLLER_REPLY: Reply = {
  kind: "line",
  text: JSON.stringify({
    ok: true,
    result: { count: 2, oldestQueuedAt: "2026-10-07T11:50:00Z" },
  }),
};

function buildCases(): Case[] {
  const cases: Case[] = [];
  const add = (...more: Case[]): void => void cases.push(...more);
  const plain = (
    name: string,
    argv: readonly string[],
    extra: Partial<Case> = {},
  ): Case => ({
    name,
    argv,
    env: BASE_ENV,
    layout: { dirs: ["home"] },
    ...extra,
  });

  // version, help and every usage error
  add(
    plain("version", ["version"]),
    plain("version-flag", ["--version"]),
    plain("version-short", ["-V"]),
    plain("version-extra-argument", ["version", "x"]),
    plain("version-flag-extra-argument", ["--version", "x"]),
    plain("version-short-extra-argument", ["-V", "--json"]),
    plain("help", ["help"]),
    plain("help-flag", ["--help"]),
    plain("help-short", ["-h"]),
    plain("help-extra-argument", ["help", "x"]),
    plain("usage-no-command", []),
    plain("usage-unknown-command", ["frobnicate"]),
    plain("usage-uppercase-command", ["VERSION"]),
    plain("usage-init-bad-flag", ["init", "--bogus"]),
    plain("usage-init-positional", ["init", "x"]),
    plain("usage-config-bare", ["config"]),
    plain("usage-config-unknown", ["config", "bogus"]),
    plain("usage-config-check-extra", ["config", "check", "x"]),
    plain("usage-config-sync-extra", ["config", "sync", "x"]),
    plain("usage-herdr-config-extra", ["herdr-config", "x"]),
    plain("usage-daemon-extra", ["daemon", "x"]),
    plain("usage-daemon-extra-flag", ["daemon", "--json"]),
    plain("usage-start-extra", ["start", "x"]),
    plain("usage-stop-extra", ["stop", "x"]),
    plain("usage-ping-extra", ["ping", "x"]),
    plain("usage-status-extra", ["status", "x"], {
      layout: project(),
      cwd: "proj",
    }),
    plain("usage-inspect-bare", ["inspect"], {
      layout: project(),
      cwd: "proj",
    }),
    plain("usage-inspect-two-ids", ["inspect", "a", "b"], {
      layout: project(),
      cwd: "proj",
    }),
    plain("usage-cancel-bare", ["cancel"]),
    plain("usage-cancel-two-ids", ["cancel", "a", "b"]),
    plain("usage-pm-bare", ["pm"]),
    plain("usage-pm-unknown", ["pm", "bogus"]),
    plain("usage-status-watch-extra", ["status", "--watch", "x"]),
    plain("usage-status-watch-bad-flag", ["status", "--watch", "--bogus"]),
    plain("usage-dash-bad-flag", ["dash", "--bogus"]),
    plain("usage-dash-extra", ["dash", "x"]),
    plain("usage-dash-bad-interval", ["dash", "--interval", "0"]),
    plain("usage-json-first", ["--json", "status"]),
    plain("herdr-config", ["herdr-config"]),
    plain("herdr-config-json", ["herdr-config", "--json"], {}),
  );

  // status --watch: a bad interval, and the first frame
  for (const [label, interval] of [
    ["zero", "0"],
    ["text", "abc"],
    ["negative", "-1"],
    ["too-big", "61"],
    ["fraction", "1.5"],
    ["leading-zero", "05"],
    ["missing", ""],
  ] as const)
    add(
      plain(
        `status-watch-bad-interval-${label}`,
        interval === ""
          ? ["status", "--watch", "--interval"]
          : ["status", "--watch", "--interval", interval],
      ),
    );
  add(
    operatorCase("status-watch-first-frame", ["status", "--watch"], {
      stopAfter: "\n---\n",
    }),
    operatorCase(
      "status-watch-first-frame-interval",
      ["status", "--watch", "--interval", "1"],
      { stopAfter: "\n---\n" },
    ),
  );

  // dash refusals
  add(
    plain("dash-not-a-terminal", ["dash"]),
    plain("dash-not-a-terminal-flags", [
      "dash",
      "--interval",
      "5",
      "--no-color",
      "--reduced-motion",
    ]),
    {
      name: "dash-not-a-terminal-agent-shell",
      argv: ["dash"],
      env: AGENT_ENV,
      layout: project(),
      cwd: "proj",
    },
    {
      name: "dash-agent-shell",
      argv: ["dash"],
      env: AGENT_ENV,
      layout: project(),
      cwd: "proj",
      tty: true,
    },
    {
      name: "dash-agent-shell-flags",
      argv: ["dash", "--no-color"],
      env: AGENT_ENV,
      layout: project(),
      cwd: "proj",
      tty: true,
    },
    {
      name: "dash-bad-flag-on-terminal",
      argv: ["dash", "--bogus"],
      env: BASE_ENV,
      layout: { dirs: ["home"] },
      tty: true,
    },
  );

  // init
  const empty = { dirs: ["proj", "home"] };
  const initCase = (
    name: string,
    argv: readonly string[],
    layout: Partial<Layout>,
    extra: Partial<Case> = {},
  ): Case => ({
    name,
    argv,
    env: GIT_ENV,
    layout,
    cwd: "proj",
    recordFiles: true,
    ...extra,
  });
  add(
    initCase("init-plain", ["init"], empty),
    initCase("init-plain-in-repository", ["init"], {
      ...empty,
      git: [{ dir: "proj", commit: true }],
    }),
    initCase("init-plain-in-empty-repository", ["init"], {
      ...empty,
      git: [{ dir: "proj", commit: false }],
    }),
    initCase("init-git", ["init", "--git"], empty),
    initCase("init-git-in-repository-with-commit", ["init", "--git"], {
      ...empty,
      git: [{ dir: "proj", commit: true }],
    }),
    initCase("init-git-in-empty-repository", ["init", "--git"], {
      ...empty,
      git: [{ dir: "proj", commit: false }],
    }),
    initCase("init-git-flag-first", ["--git", "init"], empty),
    initCase("init-existing-capstan", ["init"], {
      ...empty,
      dirs: ["proj/.capstan", "home"],
    }),
    initCase("init-existing-project", ["init"], project()),
    initCase(
      "init-git-existing-project-without-commit",
      ["init", "--git"],
      project({ git: "none" }),
    ),
    initCase(
      "init-git-existing-project-committed",
      ["init", "--git"],
      project(),
    ),
    initCase("init-kept-starter-config", ["init"], {
      ...empty,
      contents: { "proj/capstan.toml": "# mine\n" },
    }),
    initCase("init-kept-designer-prompt", ["init"], {
      ...empty,
      contents: { "proj/roles/designer.md": "# my designer\n" },
    }),
    initCase("init-git-lists-extra-files", ["init", "--git"], {
      ...empty,
      contents: {
        "proj/README.md": "# proj\n",
        "proj/src/main.c": "int main(void) { return 0; }\n",
      },
    }),
    initCase("init-no-git-binary", ["init"], empty, {
      env: { ...BASE_ENV, PATH: "" },
    }),
    initCase("init-git-no-git-binary", ["init", "--git"], empty, {
      env: { ...BASE_ENV, PATH: "" },
    }),
    initCase(
      "init-git-in-subdirectory-of-repository",
      ["init", "--git"],
      { dirs: ["outer/proj", "home"], git: [{ dir: "outer", commit: true }] },
      { cwd: "outer/proj" },
    ),
    initCase(
      "init-plain-in-subdirectory-of-repository",
      ["init"],
      { dirs: ["outer/proj", "home"], git: [{ dir: "outer", commit: true }] },
      { cwd: "outer/proj" },
    ),
    initCase("init-json-flag", ["init", "--json"], empty),
  );

  // config check and sync
  const toml = (text: string): Partial<Layout> =>
    project({ toml: text, git: "none" });
  add(
    operatorCase("config-check-starter", ["config", "check"], {
      scratch: false,
      layout: project({ git: "none" }),
    }),
    operatorCase("config-check-no-file", ["config", "check"], {
      scratch: false,
      layout: project({ bare: true, git: "none" }),
    }),
    operatorCase("config-check-invalid-toml", ["config", "check"], {
      scratch: false,
      layout: toml("[hosts.claude\nkind = \n"),
    }),
    operatorCase("config-check-unknown-role-host", ["config", "check"], {
      scratch: false,
      layout: toml(
        `${STARTER_CONFIG}\n[roles.ghost]\nkind = "Developer"\nhost = "nowhere"\n`,
      ),
    }),
    operatorCase("config-check-codex-warning", ["config", "check"], {
      scratch: false,
      layout: toml(
        `${STARTER_CONFIG}\n[hosts.codex]\nkind = "codex"\n\n[roles.coder]\nkind = "Developer"\nhost = "codex"\nprompt = "You write code."\n`,
      ),
    }),
    operatorCase("config-check-outside-a-project", ["config", "check"], {
      scratch: false,
      layout: { dirs: ["proj", "home"] },
    }),
    operatorCase("config-sync-no-controller", ["config", "sync"], {
      scratch: false,
      layout: project({ git: "none" }),
    }),
    operatorCase("config-sync-valid", ["config", "sync"], {
      scratch: false,
      layout: project({ ledger: "v37", git: "none" }),
    }),
    operatorCase("config-sync-twice", ["config", "sync"], {
      scratch: false,
      layout: project({ ledger: "v37", git: "none" }),
      before: [{ argv: ["config", "sync"] }],
    }),
    operatorCase("config-sync-invalid-toml", ["config", "sync"], {
      scratch: false,
      layout: {
        ...toml("[hosts.claude\nkind = \n"),
        copy: {
          "proj/.capstan/state/controller.sqlite": "ledgers/v37.sqlite3",
        },
      },
    }),
    operatorCase("config-sync-project-name-mismatch", ["config", "sync"], {
      scratch: false,
      layout: {
        ...project({
          ledger: "v37",
          git: "none",
          toml: STARTER_CONFIG.replace(
            "schema_version = 1\n",
            'schema_version = 1\n\n[project]\nname = "other"\n',
          ),
        }),
      },
    }),
    operatorCase("config-sync-daemon-holds-the-ledger", ["config", "sync"], {
      layout: project({ ledger: "v37" }),
      before: [{ argv: ["start"] }],
    }),
    operatorCase("config-sync-outside-a-project", ["config", "sync"], {
      scratch: false,
      layout: { dirs: ["proj", "home"] },
    }),
  );

  // start and stop against a scratch Node daemon
  const refusedReply: Reply = {
    kind: "line",
    text: JSON.stringify({
      ok: false,
      code: "unauthorized",
      message: "credential not accepted",
    }),
  };
  add(
    operatorCase("start-fresh", ["start"]),
    operatorCase("start-fresh-json", ["start", "--json"]),
    operatorCase("start-already-running", ["start"], {
      before: [{ argv: ["start"] }],
    }),
    operatorCase("start-already-running-json", ["start", "--json"], {
      before: [{ argv: ["start"] }],
    }),
    operatorCase("start-launch-failed", ["start"], {
      env: BASE_ENV,
      before: [{ argv: ["start"], env: OFF_ENV }],
    }),
    operatorCase("start-launch-failed-json", ["start", "--json"], {
      env: BASE_ENV,
      before: [{ argv: ["start"], env: OFF_ENV }],
    }),
    operatorCase("start-launch-off-without-capstan-toml", ["start"], {
      layout: project({ bare: true }),
    }),
    operatorCase("start-no-git", ["start"], {
      layout: project({ git: "none" }),
      scratch: false,
    }),
    operatorCase("start-git-without-commit", ["start"], {
      layout: project({ git: "init" }),
      scratch: false,
    }),
    operatorCase("start-no-project", ["start"], {
      layout: { dirs: ["proj", "home"] },
      scratch: false,
    }),
    operatorCase("start-unreachable", ["start"], {
      layout: project(),
      scratch: false,
      socket: SOCKET,
      reply: { kind: "hang" },
    }),
    operatorCase("start-refused", ["start"], {
      layout: project(),
      scratch: false,
      socket: SOCKET,
      reply: refusedReply,
    }),
    operatorCase("stop-running", ["stop"], { before: [{ argv: ["start"] }] }),
    operatorCase("stop-running-json", ["stop", "--json"], {
      before: [{ argv: ["start"] }],
    }),
    operatorCase("stop-not-running", ["stop"], { scratch: false }),
    operatorCase("stop-not-running-json", ["stop", "--json"], {
      scratch: false,
    }),
    operatorCase("stop-twice", ["stop"], {
      before: [{ argv: ["start"] }, { argv: ["stop"] }],
      scratch: false,
    }),
    operatorCase("stop-no-project", ["stop"], {
      layout: { dirs: ["proj", "home"] },
      scratch: false,
    }),
    operatorCase("stop-unreachable", ["stop"], {
      scratch: false,
      socket: SOCKET,
      reply: { kind: "hang" },
    }),
    operatorCase("stop-refused", ["stop"], {
      scratch: false,
      socket: SOCKET,
      reply: refusedReply,
    }),
  );

  // offline status and inspect over committed ledgers of several migration versions
  const withWork = (ledger: string): Partial<Layout> =>
    project({ ledger, git: "none" });
  for (const { name: version } of FIXTURE_LEDGERS) {
    add(
      operatorCase(`status-offline-${version}`, ["status"], {
        scratch: false,
        layout: withWork(version),
      }),
      operatorCase(`status-offline-json-${version}`, ["status", "--json"], {
        scratch: false,
        layout: withWork(version),
      }),
      operatorCase(`inspect-offline-${version}`, ["inspect", "w-1"], {
        scratch: false,
        layout: withWork(version),
      }),
      operatorCase(
        `inspect-offline-json-${version}`,
        ["inspect", "w-1", "--json"],
        {
          scratch: false,
          layout: withWork(version),
        },
      ),
      operatorCase(`inspect-offline-missing-${version}`, ["inspect", "w-404"], {
        scratch: false,
        layout: withWork(version),
      }),
    );
  }
  add(
    operatorCase("status-offline-no-ledger", ["status"], {
      scratch: false,
      layout: project({ git: "none" }),
    }),
    operatorCase("status-offline-no-ledger-json", ["status", "--json"], {
      scratch: false,
      layout: project({ git: "none" }),
    }),
    operatorCase("inspect-offline-no-ledger", ["inspect", "w-1"], {
      scratch: false,
      layout: project({ git: "none" }),
    }),
    operatorCase("status-offline-no-project", ["status"], {
      scratch: false,
      layout: { dirs: ["proj", "home"] },
    }),
    operatorCase("inspect-offline-no-project", ["inspect", "w-1"], {
      scratch: false,
      layout: { dirs: ["proj", "home"] },
    }),
    operatorCase("status-offline-key-too-open", ["status"], {
      scratch: false,
      layout: {
        ...project({ git: "none" }),
        modes: { ...project().modes, "proj/.capstan/operator.key": "0644" },
      },
    }),
    operatorCase("status-offline-wrong-key", ["status"], {
      scratch: false,
      layout: {
        ...project({ ledger: "v37", git: "none" }),
        contents: {
          ...project().contents,
          "proj/.capstan/operator.key":
            "another-operator-key-0123456789abcdefghijklmnop\n",
        },
      },
    }),
    operatorCase("status-offline-subdirectory-is-not-a-project", ["status"], {
      scratch: false,
      layout: {
        ...withWork("v37"),
        dirs: ["proj/sub", "home", "proj/.capstan/state"],
      },
      cwd: "proj/sub",
    }),
    operatorCase("status-online-fresh", ["status"]),
    operatorCase("status-online-json", ["status", "--json"]),
    operatorCase("status-online-pause", ["status"], {
      before: [{ argv: ["pause", "--reason", "break"] }],
    }),
    operatorCase("inspect-online-missing", ["inspect", "w-404"], {
      before: [{ argv: ["start"] }],
    }),
  );

  // the commands that reach a daemon as the operator: one transcript for each ROUTES command
  const routed: readonly (readonly [string, readonly string[]])[] = [
    ["ping", ["ping"]],
    ["ping-json", ["ping", "--json"]],
    ["inbox", ["inbox"]],
    ["ack", ["ack", "m-1"]],
    ["wait", ["wait"]],
    ["report", ["report", "x"]],
    ["ask", ["ask", "x"]],
    ["request-review", ["request-review", "r-1"]],
    ["integrate", ["integrate", "r-1"]],
    ["plan-show", ["plan", "show"]],
    ["plan-open", ["plan", "open", "normal", "A plan"]],
    ["op-show", ["op", "show"]],
    ["op-grants", ["op", "grants"]],
    ["link", ["link", "plan", "ref-1", "nx-1"]],
    ["review", ["review", "pass", "fine"]],
    ["finding", ["finding", "developer-1", "high", "e", "c", "d"]],
    ["observe", ["observe", "developer-1"]],
    ["prompt-show", ["prompt", "show", "developer-1"]],
    ["peek", ["peek", "developer-1"]],
    ["assign", ["assign", "w-1", "developer-1"]],
    ["cancel", ["cancel", "w-1"]],
    ["cancel-json", ["cancel", "w-1", "--json"]],
    ["send", ["send", "pm-1", "hello"]],
    ["pm-restart", ["pm", "restart"]],
    ["pm-restart-json", ["pm", "restart", "--json"]],
    ["launch", ["launch"]],
    ["pause", ["pause", "--reason", "break"]],
    ["pause-agent", ["pause", "developer-1", "--reason", "break"]],
    ["resume", ["resume", "--reason", "back"]],
    ["spawn", ["spawn", "developer"]],
    ["release", ["release", "developer-1"]],
    ["replace", ["replace", "developer-1"]],
    ["resolve", ["resolve", "m-1"]],
    ["shutdown", ["shutdown"]],
  ];
  for (const [label, argv] of routed)
    add(operatorCase(`operator-${label}`, argv));

  // the agent environment, for the commands the cli-transcript corpus does not hold
  const agentRouted: readonly (readonly [string, readonly string[], Reply])[] =
    [
      [
        "op-show",
        ["op", "show"],
        line({ ok: true, result: { proposals: [] } }),
      ],
      [
        "op-grants",
        ["op", "grants"],
        line({ ok: true, result: { grants: [] } }),
      ],
      [
        "link-plan",
        ["link", "plan", "ref-1", "nx-1"],
        line({ ok: true, result: { linked: true } }),
      ],
      [
        "finding-forbidden",
        ["finding", "developer-1", "high", "e", "c", "d"],
        line({
          ok: false,
          code: "forbidden",
          message: "the caller is not a supervisor",
        }),
      ],
      [
        "prompt-show",
        ["prompt", "show", "developer-1"],
        line({ ok: false, code: "not_found", message: "no relay" }),
      ],
    ];
  for (const [label, argv, reply] of agentRouted)
    add({
      name: `agent-${label}`,
      argv,
      env: AGENT_ENV,
      layout: { dirs: ["proj/.capstan/state", "home"] },
      cwd: "proj",
      socket: SOCKET,
      reply,
    });

  // inbox --hook, against the fake daemon
  add({
    name: "inbox-hook-waiting",
    argv: ["inbox", "--hook"],
    env: AGENT_ENV,
    layout: { dirs: ["proj/.capstan/state", "home"] },
    cwd: "proj",
    socket: SOCKET,
    reply: CONTROLLER_REPLY,
  });
  return cases;
}

function line(value: unknown): Reply {
  return { kind: "line", text: JSON.stringify(value) };
}

function substitute(text: string, scratch: string): string {
  return text.split("$ROOT").join(scratch).split("$PATH").join(HOST_PATH);
}

function fileMode(mode: string | undefined, fallback: number): number {
  return mode === undefined ? fallback : Number.parseInt(mode, 8);
}

function gitRun(directory: string, args: readonly string[]): string {
  const result = spawnSync(
    "git",
    [
      "-C",
      directory,
      "-c",
      "user.name=fixture",
      "-c",
      "user.email=fixture@example.com",
      ...args,
    ],
    {
      encoding: "utf8",
      env: {
        PATH: HOST_PATH,
        HOME: "/nonexistent",
        GIT_CONFIG_GLOBAL: "/dev/null",
      },
    },
  );
  if (result.status !== 0)
    throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
  return result.stdout;
}

/** Makes the layout under `scratch`. */
function makeLayout(layout: Layout, scratch: string): void {
  for (const dir of layout.dirs) {
    mkdirSync(path.join(scratch, dir), { recursive: true });
  }
  for (const file of layout.files) {
    mkdirSync(path.dirname(path.join(scratch, file)), { recursive: true });
    writeFileSync(path.join(scratch, file), "");
  }
  for (const [file, text] of Object.entries(layout.contents)) {
    mkdirSync(path.dirname(path.join(scratch, file)), { recursive: true });
    writeFileSync(path.join(scratch, file), substitute(text, scratch));
  }
  for (const [file, source] of Object.entries(layout.copy)) {
    mkdirSync(path.dirname(path.join(scratch, file)), { recursive: true });
    copyFileSync(
      path.join(LOCAL_TRANSCRIPT_DIRECTORY, source),
      path.join(scratch, file),
    );
    chmodSync(path.join(scratch, file), 0o600);
  }
  for (const link of layout.links)
    symlinkSync(path.join(scratch, link.to), path.join(scratch, link.path));
  for (const entry of layout.git) {
    const directory = path.join(scratch, entry.dir);
    mkdirSync(directory, { recursive: true });
    gitRun(directory, ["init", "--quiet"]);
    // What `cstan init` does for a repository: keep .capstan out of the commit.
    if (entry.commit) {
      writeFileSync(
        path.join(directory, ".git", "info", "exclude"),
        ".capstan/\n",
      );
      gitRun(directory, ["add", "-A"]);
      gitRun(directory, [
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        "chore: initial commit",
      ]);
    }
  }
  // Modes last: a directory made private must not stop the files above from being made.
  const modes = Object.entries(layout.modes).sort(
    ([a], [b]) => b.length - a.length,
  );
  for (const [file, mode] of modes) {
    const target = path.join(scratch, file);
    if (existsSync(target)) chmodSync(target, fileMode(mode, 0o600));
  }
  for (const file of [
    ...Object.keys(layout.contents),
    ...Object.keys(layout.copy),
  ])
    if (layout.modes[file] === undefined)
      chmodSync(path.join(scratch, file), 0o600);
}

interface Run {
  readonly stdout: string;
  readonly stderr: string;
  readonly exit: number | null;
  readonly signal: string | null;
}

const PTY_RUNNER = `
import os, pty, sys
pid, fd = pty.fork()
if pid == 0:
    os.execvpe(sys.argv[1], sys.argv[1:], os.environ)
out = b""
while True:
    try:
        data = os.read(fd, 65536)
    except OSError:
        break
    if not data:
        break
    out += data
_, status = os.waitpid(pid, 0)
sys.stdout.buffer.write(out)
sys.stdout.flush()
sys.exit(os.waitstatus_to_exitcode(status))
`;

function nodeArguments(
  cli: string,
  argv: readonly string[],
  shim: boolean,
): string[] {
  const clock = `data:text/javascript,${encodeURIComponent(`Date.now=()=>${NOW};`)}`;
  return [...(shim ? [`--import=${clock}`] : []), cli, ...argv];
}

async function runCommand(
  cli: string,
  argv: readonly string[],
  env: Readonly<Record<string, string>>,
  cwd: string,
  options: {
    readonly shim: boolean;
    readonly stdin: string;
    readonly tty: boolean;
    readonly stopAfter: string | null;
  },
): Promise<Run> {
  const [command, args] = options.tty
    ? [
        "/usr/bin/python3",
        [
          "-c",
          PTY_RUNNER,
          process.execPath,
          ...nodeArguments(cli, argv, options.shim),
        ],
      ]
    : [process.execPath, nodeArguments(cli, argv, options.shim)];
  const child = spawn(command, args, {
    cwd,
    env: { ...env },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stdin.on("error", () => undefined);
  child.stdin.end(options.stdin);
  const out: Buffer[] = [];
  const err: Buffer[] = [];
  let stopped = false;
  child.stdout.on("data", (chunk: Buffer) => {
    out.push(chunk);
    if (
      options.stopAfter !== null &&
      !stopped &&
      Buffer.concat(out).toString("utf8").includes(options.stopAfter)
    ) {
      stopped = true;
      child.kill("SIGTERM");
    }
  });
  child.stderr.on("data", (chunk: Buffer) => err.push(chunk));
  const limit = setTimeout(() => child.kill("SIGKILL"), STEP_LIMIT_MS);
  const [exit, signal] = await new Promise<[number | null, string | null]>(
    (resolve) => child.once("close", (code, sig) => resolve([code, sig])),
  );
  clearTimeout(limit);
  if (options.stopAfter !== null && !stopped)
    throw new Error(
      `${argv.join(" ")} never printed the text it is stopped at`,
    );
  return {
    stdout: Buffer.concat(out).toString("utf8"),
    stderr: Buffer.concat(err).toString("utf8"),
    exit,
    signal,
  };
}

/** Listens on `socketPath`; reads one request line and answers it as `reply` says. */
async function fakeDaemon(
  socketPath: string,
  reply: Reply,
): Promise<{ request: () => string | null; close: () => Promise<void> }> {
  let request: string | null = null;
  const connections = new Set<net.Socket>();
  const server = net.createServer((connection) => {
    connections.add(connection);
    connection.on("error", () => undefined);
    let bytes = Buffer.alloc(0);
    connection.on("data", (chunk: Buffer) => {
      if (request !== null) return;
      bytes = Buffer.concat([bytes, chunk]);
      const newline = bytes.indexOf(10);
      if (newline < 0) return;
      request = bytes.subarray(0, newline).toString("utf8");
      if (reply.kind === "line") connection.write(`${reply.text}\n`);
      else if (reply.kind === "close") connection.end();
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
  return {
    request: () => request,
    close: async () => {
      for (const connection of connections) connection.destroy();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

/** Every file below `directory`, with its mode and (for a file) its text; `.git` is a single directory entry. */
function snapshot(
  directory: string,
): Map<string, { kind: "dir" | "file"; mode: string; bytes?: Buffer }> {
  const found = new Map<
    string,
    { kind: "dir" | "file"; mode: string; bytes?: Buffer }
  >();
  const walk = (current: string): void => {
    for (const name of readdirSync(current).sort()) {
      const full = path.join(current, name);
      const relative = path.relative(directory, full);
      const stat = lstatSync(full);
      const mode = (stat.mode & 0o777).toString(8).padStart(4, "0");
      if (stat.isDirectory()) {
        found.set(relative, { kind: "dir", mode });
        if (name !== ".git") walk(full);
      } else if (stat.isFile()) {
        found.set(relative, { kind: "file", mode, bytes: readFileSync(full) });
      }
    }
  };
  walk(directory);
  return found;
}

/** What `stop` of a scratch daemon needs to run: the case environment of the project, or nothing. */
async function stopScratchDaemon(
  cli: string,
  scratch: string,
  env: Readonly<Record<string, string>>,
  cwd: string,
): Promise<void> {
  const socket = path.join(
    scratch,
    "proj",
    ".capstan",
    "state",
    "control.sock",
  );
  if (!existsSync(socket)) return;
  await runCommand(cli, ["stop"], env, cwd, {
    shim: false,
    stdin: "",
    tty: false,
    stopAfter: null,
  }).catch(() => undefined);
  // A daemon that did not stop is ours (started from this scratch directory): find it by its working directory.
  for (const entry of readdirSync("/proc")) {
    if (!/^\d+$/.test(entry)) continue;
    try {
      const cwdLink = realpathSync(`/proc/${entry}/cwd`);
      if (cwdLink === path.join(scratch, "proj"))
        process.kill(Number(entry), "SIGKILL");
    } catch {
      // The process is gone or not ours to look at.
    }
  }
}

function fixed(
  env: Readonly<Record<string, string>>,
  scratch: string,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(env).map(([key, value]) => [
      key,
      substitute(value, scratch),
    ]),
  );
}

class DummyMembers {
  static readonly value = {
    pm: {
      agentId: "none",
      actorId: "\u0000no-pm-actor",
      token: "\u0000no-pm-token",
    },
    developer: {
      agentId: "none",
      actorId: "\u0000no-developer-actor",
      token: "\u0000no-developer-token",
    },
  } as const;
}

function normalise(
  text: string,
  scratch: string,
  normaliser: Normaliser,
  secrets: readonly string[],
  version: string,
): string {
  let result = text;
  for (const secret of secrets)
    result = result.split(secret).join("<TOKEN:operator>");
  return normaliseVersion(normaliser.text(result), version);
}

async function record(
  source: Case,
  settings: Required<Pick<ExportOptions, "cli" | "version">>,
): Promise<LocalTranscript> {
  const scratch = realpathSync(
    mkdtempSync(path.join(os.tmpdir(), "capstan-freeze-corpus-")),
  );
  const layout = emptyLayout(source.layout);
  const cwdName = source.cwd ?? ".";
  const cwd = path.join(scratch, cwdName);
  const env = source.env ?? BASE_ENV;
  const hasDaemon = source.scratch === true;
  try {
    makeLayout(layout, scratch);
    mkdirSync(cwd, { recursive: true });
    const before = source.before ?? [];
    const options = {
      shim: !hasDaemon,
      stdin: source.stdin ?? "",
      tty: source.tty === true,
      stopAfter: source.stopAfter ?? null,
    };
    const fake =
      source.socket !== undefined && source.reply !== undefined
        ? await fakeDaemon(substitute(source.socket, scratch), source.reply)
        : undefined;
    const original = snapshot(scratch);
    let observed: Run;
    const normaliser = new Normaliser(scratch, DummyMembers.value);
    try {
      for (const step of before)
        await runCommand(
          settings.cli,
          step.argv,
          fixed(step.env ?? env, scratch),
          cwd,
          {
            shim: false,
            stdin: "",
            tty: false,
            stopAfter: null,
          },
        );
      observed = await runCommand(
        settings.cli,
        source.argv.map((arg) => substitute(arg, scratch)),
        fixed(env, scratch),
        cwd,
        options,
      );
    } finally {
      await fake?.close();
      if (hasDaemon || before.length > 0)
        await stopScratchDaemon(
          settings.cli,
          scratch,
          fixed(env, scratch),
          cwd,
        );
    }
    const secrets: string[] = [];
    for (const keyFile of [
      path.join(scratch, "proj", ".capstan", "operator.key"),
      path.join(cwd, ".capstan", "operator.key"),
    ])
      if (existsSync(keyFile)) {
        const key = readFileSync(keyFile, "utf8").trim();
        if (key !== "" && !secrets.includes(key)) secrets.push(key);
      }
    let files: FileEntry[] | null = null;
    let commits: Record<string, { subject: string; files: string[] }[]> | null =
      null;
    if (source.recordFiles === true) {
      const after = snapshot(scratch);
      files = [];
      for (const [relative, entry] of after) {
        const was = original.get(relative);
        if (
          was !== undefined &&
          was.kind === entry.kind &&
          was.mode === entry.mode &&
          (entry.bytes === undefined || was.bytes?.equals(entry.bytes) === true)
        )
          continue;
        if (relative.startsWith(".git/") && relative !== ".git/info/exclude")
          continue;
        files.push({
          path: normalise(
            relative,
            scratch,
            normaliser,
            secrets,
            settings.version,
          ),
          kind: entry.kind,
          mode: entry.mode,
          ...(entry.bytes === undefined
            ? {}
            : {
                text: normalise(
                  entry.bytes.toString("utf8"),
                  scratch,
                  normaliser,
                  secrets,
                  settings.version,
                ),
              }),
        });
      }
      commits = {};
      for (const entry of [...layout.git.map((g) => g.dir), cwdName]) {
        const directory = path.join(scratch, entry);
        if (!existsSync(path.join(directory, ".git"))) continue;
        const log = spawnSync(
          "git",
          ["-C", directory, "log", "--format=%H%x00%s"],
          { encoding: "utf8", env: { PATH: HOST_PATH, HOME: "/nonexistent" } },
        );
        const entries: { subject: string; files: string[] }[] = [];
        if (log.status === 0)
          for (const row of log.stdout.split("\n").filter((l) => l !== "")) {
            const [hash, subject] = row.split("\0") as [string, string];
            const tree = spawnSync(
              "git",
              ["-C", directory, "ls-tree", "-r", "--name-only", hash],
              {
                encoding: "utf8",
                env: { PATH: HOST_PATH, HOME: "/nonexistent" },
              },
            );
            entries.push({
              subject,
              files: tree.stdout.split("\n").filter((l) => l !== ""),
            });
          }
        commits[entry] = entries;
      }
    }
    const text = (value: string): string =>
      normalise(value, scratch, normaliser, secrets, settings.version);
    return {
      name: source.name,
      argv: source.argv,
      env,
      layout,
      cwd: cwdName,
      stdin: source.stdin ?? "",
      tty: source.tty === true,
      now: NOW,
      before,
      daemon: hasDaemon
        ? { kind: "scratch" }
        : source.socket !== undefined && source.reply !== undefined
          ? { kind: "fake", socket: source.socket, reply: source.reply }
          : null,
      stopAfter: source.stopAfter ?? null,
      request: fake === undefined ? null : text(fake.request() ?? "") || null,
      node: {
        stdout: text(observed.stdout),
        stderr: text(observed.stderr),
        exit: observed.exit,
        signal: observed.signal,
      },
      files,
      commits,
    };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

/** Every transcript by file name, as the text the exporter writes. */
export async function exportLocalTranscripts(
  options: ExportOptions = {},
): Promise<Map<string, string>> {
  const settings = {
    cli: options.cli ?? CLI,
    version: options.version ?? PACKAGE_VERSION,
  };
  const cases = buildCases().filter(
    (source) => options.only === undefined || options.only(source.name),
  );
  const names = new Set<string>();
  for (const source of cases) {
    if (names.has(source.name))
      throw new Error(`duplicate case ${source.name}`);
    names.add(source.name);
  }
  const transcripts = new Array<LocalTranscript>(cases.length);
  let next = 0;
  const worker = async (): Promise<void> => {
    for (;;) {
      const at = next++;
      const source = cases[at];
      if (source === undefined) return;
      transcripts[at] = await record(source, settings);
    }
  };
  await Promise.all(Array.from({ length: 4 }, worker));
  return new Map(
    transcripts.map((t) => [
      `${t.name}.json`,
      `${JSON.stringify(t, null, 2)}\n`,
    ]),
  );
}

// ---- the ledgers the offline status and inspect cases read ---------------------------------------------------------

const INPUT_KINDS = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

/** A ledger at the current schema with a project, its operator, a PM seat and one work item `w-1`. */
async function currentLedger(directory: string): Promise<void> {
  mkdirSync(directory, { recursive: true, mode: 0o700 });
  const core = await ControllerCore.open({
    stateDirectory: directory,
    project: {
      projectId: PROJECT_ID,
      name: "proj",
      ownerCredential: OPERATOR_KEY,
      initialInputs: INPUT_KINDS.map((kind) => ({
        kind,
        content:
          kind === "acceptance_criteria"
            ? ["criterion"]
            : { kind, revision: 1 },
      })),
    },
  });
  core.close();
  const database = openSqlite(path.join(directory, "controller.sqlite"));
  try {
    insertWorkItem(database, PROJECT_ID, {
      workItemId: "w-1",
      title: "First work item",
      state: "pending",
    });
  } finally {
    database.close();
  }
}

/** `full` cut back to the first `count` migrations, keeping the rows of every table both schemas have. */
function olderLedger(full: string, out: string, count: number): void {
  rmSync(out, { force: true });
  const database = openSqlite(out);
  try {
    const files = readdirSync(MIGRATION_DIRECTORY)
      .filter((file) => file.endsWith(".sql"))
      .sort();
    for (const file of files.slice(0, count)) {
      const bytes = readFileSync(path.join(MIGRATION_DIRECTORY, file));
      database.exec("BEGIN");
      database.exec(bytes.toString("utf8"));
      database
        .prepare(
          "INSERT INTO schema_migrations(version, name, checksum, applied_at) VALUES (?, ?, ?, ?)",
        )
        .run(
          Number(file.slice(0, 4)),
          file,
          createHash("sha256").update(bytes).digest("hex"),
          "2026-01-01T00:00:00.000Z",
        );
      database.exec("COMMIT");
    }
    database.exec(`ATTACH '${full.replaceAll("'", "''")}' AS source`);
    database.exec("PRAGMA foreign_keys = OFF");
    const names = (schema: string, table: string): string[] =>
      (
        database
          .prepare(
            `SELECT name FROM pragma_table_info('${table}', '${schema}')`,
          )
          .all() as { name: string }[]
      ).map((row) => row.name);
    const tables = (
      database
        .prepare(
          "SELECT name FROM main.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> 'schema_migrations'",
        )
        .all() as { name: string }[]
    ).map((row) => row.name);
    const sourceTables = new Set(
      (
        database
          .prepare("SELECT name FROM source.sqlite_master WHERE type = 'table'")
          .all() as { name: string }[]
      ).map((row) => row.name),
    );
    for (const table of tables) {
      if (!sourceTables.has(table)) continue;
      const wanted = new Set(names("source", table));
      const columns = names("main", table).filter((column) =>
        wanted.has(column),
      );
      if (columns.length === 0) continue;
      const list = columns.join(", ");
      database.exec(
        `INSERT OR IGNORE INTO main.${table}(${list}) SELECT ${list} FROM source.${table}`,
      );
    }
    database.exec("DETACH source");
    database.exec("VACUUM");
  } finally {
    database.close();
  }
}

/** Rebuilds the committed ledgers. Only run on purpose: the transcripts that read them are exported again afterwards. */
export async function buildLedgers(): Promise<void> {
  const work = realpathSync(
    mkdtempSync(path.join(os.tmpdir(), "capstan-freeze-corpus-")),
  );
  try {
    mkdirSync(LEDGER_DIRECTORY, { recursive: true });
    const state = path.join(work, "state");
    await currentLedger(state);
    const full = path.join(state, "controller.sqlite");
    const database = openSqlite(full);
    database.exec("PRAGMA wal_checkpoint(TRUNCATE)");
    database.close();
    for (const { name, migrations } of FIXTURE_LEDGERS) {
      const target = path.join(LEDGER_DIRECTORY, `${name}.sqlite3`);
      if (name === "v37") copyFileSync(full, target);
      else olderLedger(full, target, migrations);
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

if (import.meta.filename === process.argv[1]) {
  if (process.argv.includes("--ledgers")) {
    await buildLedgers();
    process.stdout.write(`wrote the ledgers to ${LEDGER_DIRECTORY}\n`);
  } else {
    const files = await exportLocalTranscripts();
    mkdirSync(LOCAL_TRANSCRIPT_DIRECTORY, { recursive: true });
    for (const [name, text] of files)
      writeFileSync(path.join(LOCAL_TRANSCRIPT_DIRECTORY, name), text);
    process.stdout.write(
      `wrote ${files.size} transcripts to ${LOCAL_TRANSCRIPT_DIRECTORY}\n`,
    );
  }
}

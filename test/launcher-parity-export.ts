/**
 * Writes the fixtures the Rust launcher (rust/crates/launcher) is tested against:
 * rust/crates/launcher/tests/parity/<group>.json for every test/launcher-sequences/<group>.json. Each sequence runs the
 * real Node `Launcher` over the stub adapter of test/launcher-stubs.ts, a real `ControllerCore` (seeded time and
 * randomness, test/kernel-parity-hooks.ts) and real git in a scratch repository, and records, per step, what the launcher
 * returned or threw, every call it made to the adapter, every git command it ran (through a `git` shim first on PATH),
 * the log events it wrote, and what changed in the ledger and in git. The Rust replay (tests/parity.rs) runs the same
 * steps on `capstan-launcher` over its own stub and compares everything.
 *
 *   npm run build && node dist/test/launcher-parity-export.js [--out <dir>]
 *
 * writes the repository's committed files, or the same tree under <dir>. launcher-parity.test.ts fails while the
 * committed files differ from a fresh export. The step language is in test/launcher-sequences/README.md.
 */
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import {
  chmodSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  readSync,
  realpathSync,
  openSync,
  closeSync,
  fstatSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { newContext } from "../src/context.js";
import { ControllerCore } from "../src/controller/core.js";
import { openDatabase } from "../src/controller/database.js";
import {
  AgentPaneMismatch,
  PaneGone,
  PaneLost,
  PhaseError,
  PromptUnrecognized,
  ShellNotReady,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import {
  Launcher,
  LauncherError,
  type LauncherAdapter,
  type SetupOutcome,
} from "../src/launcher.js";
import {
  config as baseConfig,
  hashOf,
  withArchitect,
  withOperator,
  withResearcher,
} from "./launcher-harness.js";
import { dumpTables, diffTables, packedText } from "./kernel-parity-export.js";
import type { Tables } from "./kernel-parity-export.js";
import { installHooks } from "./kernel-parity-hooks.js";
import { StubAdapter } from "./launcher-stubs.js";
import { removeTempDir, tempDir } from "./tmp.js";
import type { CapstanConfig } from "../src/config/capstan-config.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const SEQUENCES_DIRECTORY = path.join(
  root,
  "test",
  "launcher-sequences",
);
export const PARITY_DIRECTORY = path.join(
  root,
  "rust",
  "crates",
  "launcher",
  "tests",
  "parity",
);

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** The wall clock the launcher's budgets read: the seeded clock must only be read by the ledger. */
const REAL_NOW = Date.now.bind(Date);

const OWNER_CREDENTIAL =
  "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";
const PROJECT = {
  projectId: "proj1",
  name: "Parity Project",
  ownerCredential: OWNER_CREDENTIAL,
  initialInputs: [
    { kind: "project_config", content: { name: "parity" } },
    { kind: "task_brief", content: { objective: "prove parity" } },
    { kind: "acceptance_criteria", content: { criteria: ["it matches"] } },
    { kind: "policy", content: { review: "required" } },
    { kind: "plan", content: { steps: ["one", "two"] } },
  ],
};

/** Strings longer than this are recorded as their SHA-256 (prompts, seeds): their bytes are compared, not shown. */
const LONG_STRING = 1500;

// ------------------------------------------------------------------------------------------------ sequences

export interface ConfigSpec {
  readonly maxWorkers?: number;
  readonly layout?: Record<string, unknown>;
  readonly pass?: string[];
  readonly hostOf?: Record<string, "codex" | "omp">;
  readonly architect?: { readonly counts: boolean };
  readonly operator?: {
    readonly counts: boolean;
    readonly enabled: boolean;
    readonly table?: boolean;
  };
  readonly researcher?: { readonly enabled: boolean };
  readonly worktree?: {
    readonly setup?: string;
    readonly setupTimeoutSeconds: number;
    readonly teardown?: string;
    readonly teardownTimeoutSeconds?: number;
  };
  readonly promptRelay?: boolean;
  /** Variables added to the daemon's environment the launcher is given. */
  readonly base?: Record<string, string>;
  /** False: no role is synced into the ledger. */
  readonly synced?: boolean;
}

export interface Step {
  readonly op: string;
  readonly as?: string;
  readonly [key: string]: unknown;
}

export interface SequenceSource {
  readonly name: string;
  readonly seed?: string;
  readonly config?: ConfigSpec;
  readonly steps: Step[];
}

function sorted(value: unknown): Json {
  if (value === undefined) return null;
  if (value === null || typeof value !== "object") return value as Json;
  if (Array.isArray(value)) return value.map(sorted);
  const out: { [key: string]: Json } = {};
  for (const key of Object.keys(value).sort()) {
    const entry = (value as Record<string, unknown>)[key];
    if (entry !== undefined) out[key] = sorted(entry);
  }
  return out;
}

/**
 * A value as the fixtures record it: keys in order, functions as `<fn>`, the scratch directory as `<tmp>`, and a long
 * string as its SHA-256 and byte length.
 */
function normal(value: unknown, scratch: string): Json {
  if (value === undefined) return null;
  if (typeof value === "function") return "<fn>";
  if (typeof value === "string") {
    const text = value.split(scratch).join("<tmp>");
    return text.length > LONG_STRING
      ? {
          bytes: Buffer.byteLength(text, "utf8"),
          sha256: createHash("sha256").update(text, "utf8").digest("hex"),
        }
      : text;
  }
  if (value === null || typeof value !== "object") return value as Json;
  if (Array.isArray(value)) return value.map((v) => normal(v, scratch));
  const out: { [key: string]: Json } = {};
  for (const key of Object.keys(value).sort()) {
    const entry = (value as Record<string, unknown>)[key];
    if (entry !== undefined) out[key] = normal(entry, scratch);
  }
  return out;
}

function lookup(value: unknown, parts: string[], reference: string): unknown {
  let current: unknown = value;
  for (const part of parts) {
    if (current === null || typeof current !== "object")
      throw new Error(`${reference} does not resolve`);
    current = (current as Record<string, unknown>)[part];
  }
  if (current === undefined) throw new Error(`${reference} does not resolve`);
  return current;
}

/** Replaces `"$name.path"` strings by the named result. */
function resolve(value: unknown, bindings: Map<string, unknown>): unknown {
  if (typeof value === "string" && /^\$[A-Za-z]/.test(value)) {
    const [name = "", ...parts] = value.slice(1).split(".");
    if (!bindings.has(name)) throw new Error(`unknown reference ${value}`);
    return lookup(bindings.get(name), parts, value);
  }
  if (Array.isArray(value)) return value.map((v) => resolve(v, bindings));
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, v] of Object.entries(value))
      out[key] = resolve(v, bindings);
    return out;
  }
  return value;
}

// ------------------------------------------------------------------------------------------------ the world

interface FailureSpec {
  readonly kind: string;
  readonly code?: string;
  readonly message: string;
}

function makeError(spec: FailureSpec): Error {
  switch (spec.kind) {
    case "herdr":
      return new HerdrError(spec.code ?? "herdr_error", spec.message);
    case "pane_gone":
      return new PaneGone(spec.message);
    case "agent_pane_mismatch":
      return new AgentPaneMismatch(spec.message);
    case "pane_lost":
      return new PaneLost(spec.message);
    case "phase":
      return new PhaseError(spec.message);
    case "prompt_unrecognized":
      return new PromptUnrecognized(spec.message);
    case "shell_not_ready":
      return new ShellNotReady(spec.message);
    default:
      return new Error(spec.message);
  }
}

/** The adapter methods whose calls are recorded; the registry reads (`paneForAgent`, `paneEntry`) are not. */
const RECORDED = new Set([
  "createWorkspace",
  "createWorktree",
  "createTab",
  "paneLayout",
  "placePane",
  "panesAtPath",
  "prepareShell",
  "startAgent",
  "answerTrustDialog",
  "closePane",
  "paneIdentity",
  "adoptPane",
  "adoptShellPane",
  "reportMetadata",
  "renameWorkspace",
  "renameTab",
  "forgetPane",
  "runInPane",
  "writePromptFile",
  "readScreen",
  "capturePrompt",
  "answerPrompt",
  "interruptWorking",
  "agentObservation",
]);
const SYNCHRONOUS = new Set(["writePromptFile", "forgetPane"]);

interface World {
  readonly scratch: string;
  readonly projectRoot: string;
  readonly realGit: string;
  readonly gitLog: string;
  readonly adapter: StubAdapter;
  readonly calls: Json[];
  readonly failures: Map<string, FailureSpec[]>;
  readonly events: Json[];
  readonly sleeps: number[];
  readonly setupCalls: Json[];
  readonly setupOutcomes: SetupOutcome[];
  readonly teardownOutcomes: SetupOutcome[];
  readonly tokens: Map<string, string>;
}

function gitEnvironment(): NodeJS.ProcessEnv {
  return {
    PATH: process.env.PATH ?? "/usr/bin:/bin",
    LC_ALL: "C",
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_CONFIG_GLOBAL: "/dev/null",
    GIT_TERMINAL_PROMPT: "0",
    GIT_AUTHOR_NAME: "Parity",
    GIT_AUTHOR_EMAIL: "parity@example.com",
    GIT_COMMITTER_NAME: "Parity",
    GIT_COMMITTER_EMAIL: "parity@example.com",
    GIT_AUTHOR_DATE: "2026-01-01T00:00:00Z",
    GIT_COMMITTER_DATE: "2026-01-01T00:00:00Z",
  };
}

function git(world: World, cwd: string, args: string[]): string {
  return execFileSync(world.realGit, args, {
    cwd,
    encoding: "utf8",
    env: gitEnvironment(),
    stdio: ["ignore", "pipe", "pipe"],
  });
}

/** The adapter's recording proxy: every call is logged, and a scripted failure is thrown before the stub is reached. */
function recordingAdapter(world: World): LauncherAdapter {
  const inner = world.adapter;
  return new Proxy(inner, {
    get(target, property) {
      const member = Reflect.get(target, property, target) as unknown;
      if (typeof property !== "string" || typeof member !== "function")
        return member;
      const method = member as (...args: unknown[]) => unknown;
      if (!RECORDED.has(property)) return method.bind(target);
      const record = (args: unknown[]): void => {
        world.calls.push(
          normal({ m: property, a: args }, world.scratch) as Json,
        );
      };
      const failure = (): Error | undefined => {
        const spec = world.failures.get(property)?.shift();
        return spec === undefined ? undefined : makeError(spec);
      };
      const afterwards = (args: unknown[], result: unknown): void => {
        // Herdr makes the worktree on disk; the stub only names it.
        if (property === "createWorktree") {
          const input = args[0] as { branch: string; base?: string };
          const created = result as { path: string };
          git(world, world.projectRoot, [
            "worktree",
            "add",
            "-b",
            input.branch,
            created.path,
            ...(input.base === undefined ? [] : [input.base]),
          ]);
        }
        if (property === "startAgent") {
          const input = args[0] as {
            name: string;
            environment?: Record<string, string>;
          };
          const token = input.environment?.CAPSTAN_TOKEN;
          if (token !== undefined) world.tokens.set(input.name, token);
        }
      };
      if (SYNCHRONOUS.has(property))
        return (...args: unknown[]) => {
          record(args);
          const error = failure();
          if (error !== undefined) throw error;
          const result = method.apply(target, args);
          afterwards(args, result);
          return result;
        };
      return async (...args: unknown[]) => {
        record(args);
        const error = failure();
        if (error !== undefined) throw error;
        const result = await method.apply(target, args);
        afterwards(args, result);
        return result;
      };
    },
  }) as unknown as LauncherAdapter;
}

function configOf(spec: ConfigSpec): CapstanConfig {
  const base = baseConfig(
    true,
    spec.maxWorkers ?? 3,
    (spec.layout ?? {}) as never,
    spec.pass ?? [],
    spec.hostOf ?? {},
  );
  const withRoles = withResearcher(
    withOperator(withArchitect(base, spec.architect), spec.operator),
    spec.researcher,
  );
  return {
    ...withRoles,
    ...(spec.worktree === undefined ? {} : { worktree: spec.worktree }),
    ...(spec.promptRelay === undefined
      ? {}
      : {
          promptRelay: {
            present: true,
            enabled: spec.promptRelay,
            captureTtlSeconds: 300,
          },
        }),
  } as unknown as CapstanConfig;
}

/**
 * The ledger as `dumpTables` has it, with the request hashes left out: a hash covers the arguments of a mutation, and those
 * name the scratch directory, which differs from one run to the next.
 */
function dumpOf(directory: string): Tables {
  const tables = dumpTables(
    path.join(directory, "controller.sqlite"),
  ) as unknown as Tables;
  const requests = tables.mutation_requests;
  const column = requests?.columns.indexOf("request_hash") ?? -1;
  if (requests !== undefined && column >= 0)
    for (const row of requests.rows) row[column] = "<hash>";
  return tables;
}

/** The log of the git shim from `offset` on, one entry per command. */
function readFrom(file: string, offset: number): { text: string; end: number } {
  const descriptor = openSync(file, "r");
  try {
    const end = fstatSync(descriptor).size;
    const buffer = Buffer.alloc(end - offset);
    readSync(descriptor, buffer, 0, buffer.length, offset);
    return { text: buffer.toString("utf8"), end };
  } finally {
    closeSync(descriptor);
  }
}

/** Branches and worktrees as git has them now. */
function gitState(world: World): Json {
  const worktrees = git(world, world.projectRoot, [
    "worktree",
    "list",
    "--porcelain",
  ])
    .split("\n\n")
    .map((block) => block.trim())
    .filter((block) => block !== "")
    .map((block) => block.split("\n"))
    .sort((a, b) => ((a[0] ?? "") < (b[0] ?? "") ? -1 : 1));
  const refs = git(world, world.projectRoot, [
    "for-each-ref",
    "--format=%(refname) %(objectname)",
    "refs/heads",
    "refs/capstan",
  ])
    .split("\n")
    .filter((line) => line !== "");
  return normal({ worktrees, refs }, world.scratch);
}

async function runSequence(source: SequenceSource): Promise<Json> {
  const seed = source.seed ?? source.name;
  const spec = source.config ?? {};
  const scratch = realpathSync(tempDir("capstan-launcher-parity-"));
  const projectRoot = path.join(scratch, "project");
  const stateDirectory = path.join(projectRoot, ".capstan", "state");
  const binDirectory = path.join(scratch, "bin");
  const promptsDirectory = path.join(scratch, "prompts");
  const worktreesDirectory = path.join(scratch, "worktrees");
  for (const directory of [
    projectRoot,
    binDirectory,
    promptsDirectory,
    worktreesDirectory,
  ])
    mkdirSync(directory, { recursive: true });
  mkdirSync(stateDirectory, { recursive: true, mode: 0o700 });
  const realGit = execFileSync("sh", ["-c", "command -v git"], {
    encoding: "utf8",
  }).trim();
  const gitLog = path.join(scratch, "git-argv.log");
  writeFileSync(gitLog, "");
  writeFileSync(
    path.join(binDirectory, "git"),
    `#!/bin/sh\nprintf '%s\\n' "$*" >> '${gitLog}'\nexec '${realGit}' "$@"\n`,
    { mode: 0o755 },
  );
  chmodSync(path.join(binDirectory, "git"), 0o755);

  const adapter = new StubAdapter();
  // The stub made its own prompt directory; the sequence's lives in the scratch directory.
  adapter.dir = promptsDirectory;
  adapter.worktreeBase = worktreesDirectory;
  const world: World = {
    scratch,
    projectRoot,
    realGit,
    gitLog,
    adapter,
    calls: [],
    failures: new Map(),
    events: [],
    sleeps: [],
    setupCalls: [],
    setupOutcomes: [],
    teardownOutcomes: [],
    tokens: new Map(),
  };
  git(world, projectRoot, ["init", "--quiet", "-b", "main"]);
  writeFileSync(path.join(projectRoot, "README.md"), "parity\n");
  git(world, projectRoot, ["add", "-A"]);
  git(world, projectRoot, ["commit", "--quiet", "-m", "chore: initial commit"]);

  const savedPath = process.env.PATH;
  const savedFrontEnd = process.env.CSTAN_FRONT_END;
  process.env.PATH = `${binDirectory}:${savedPath ?? "/usr/bin:/bin"}`;
  delete process.env.CSTAN_FRONT_END;
  const hooks = installHooks(seed);
  let core: ControllerCore | undefined;
  try {
    // The ledger's migration timestamps come from the system clock in both implementations; migrating first keeps them
    // out of the seeded clock.
    (
      await openDatabase(path.join(stateDirectory, "controller.sqlite"))
    ).close();
    core = await hooks.run(() =>
      ControllerCore.open({ stateDirectory, project: PROJECT as never }),
    );
    const open = core;
    if (spec.synced !== false)
      hooks.run(() =>
        open.syncRoleDefinitions(
          newContext(open, OWNER_CREDENTIAL),
          [
            "pm:PM",
            "pm2:PM",
            "developer:Developer",
            "developer2:Developer",
            "architect:Developer",
            "operator:Developer",
            "researcher:Developer",
            "supervisor:Supervisor",
          ].map((entry) => {
            const [name = "", kind = ""] = entry.split(":");
            return {
              name,
              kind: kind as "PM" | "Developer" | "Supervisor",
              host: spec.hostOf?.[name] ?? "claude",
              configHash: hashOf(name),
            };
          }),
        ),
      );
    const proxied = recordingAdapter(world);
    const make = (): Launcher =>
      new Launcher({
        core: open,
        adapter: proxied,
        config: configOf(spec),
        projectRoot,
        cliPath: "/opt/capstan/cli.js",
        socketPath: path.join(stateDirectory, "control.sock"),
        credential: OWNER_CREDENTIAL,
        nodePath: "/usr/bin/node",
        baseEnvironment: {
          PATH: "/usr/bin:/bin",
          HOME: "/home/x",
          LANG: "C",
          SECRET: "no",
          ...spec.base,
        },
        now: REAL_NOW,
        sleep: async (milliseconds) => {
          world.sleeps.push(milliseconds);
        },
        log: (event, details) => {
          world.events.push(normal({ event, details }, scratch));
        },
        runSetup: async (command, cwd, timeoutMs) => {
          world.setupCalls.push(
            normal({ kind: "setup", command, cwd, timeoutMs }, scratch),
          );
          return world.setupOutcomes.shift() ?? { status: "ok" };
        },
        runTeardown: async (command, cwd, timeoutMs, environment) => {
          world.setupCalls.push(
            normal(
              { kind: "teardown", command, cwd, timeoutMs, environment },
              scratch,
            ),
          );
          return world.teardownOutcomes.shift() ?? { status: "ok" };
        },
      });
    let launcher = make();
    const bindings = new Map<string, unknown>();
    const records: Json[] = [];
    let gitOffset = 0;
    let previous = dumpOf(stateDirectory);

    for (const [index, raw] of source.steps.entries()) {
      const step = resolve(raw, bindings) as Step;
      const record: { [key: string]: Json } = {
        op: step.op,
        step: index,
        args: sorted(
          Object.fromEntries(
            Object.entries(raw).filter(([k]) => k !== "op" && k !== "as"),
          ),
        ),
        ...(raw.as === undefined ? {} : { as: raw.as }),
      };
      world.calls.length = 0;
      world.events.length = 0;
      world.sleeps.length = 0;
      world.setupCalls.length = 0;
      let result: unknown;
      let failure: { [key: string]: Json } | undefined;
      try {
        result = await perform(step, launcher, world, open, hooks, () => {
          launcher = make();
        });
      } catch (error) {
        failure =
          error instanceof LauncherError
            ? { code: error.code, message: error.message, name: error.name }
            : {
                message: error instanceof Error ? error.message : String(error),
                name: error instanceof Error ? error.name : "Error",
              };
      }
      if (failure === undefined) {
        const plain: unknown =
          result === undefined ? null : JSON.parse(JSON.stringify(result));
        record.result = normal(plain, scratch);
        if (raw.as !== undefined) bindings.set(raw.as, plain);
      } else record.error = normal(failure, scratch);
      const shim = readFrom(gitLog, gitOffset);
      gitOffset = shim.end;
      record.git = normal(
        shim.text.split("\n").filter((line) => line !== ""),
        scratch,
      );
      record.calls = [...world.calls];
      record.events = [...world.events];
      if (world.sleeps.length > 0) record.sleeps = [...world.sleeps];
      if (world.setupCalls.length > 0) record.commands = [...world.setupCalls];
      record.state = gitState(world);
      const now = dumpOf(stateDirectory);
      record.tablesDiff = normal(diffTables(previous, now), scratch);
      previous = now;
      records.push(record);
    }
    return sorted({
      name: source.name,
      seed,
      config: spec,
      steps: records,
    });
  } finally {
    hooks.restore();
    try {
      core?.close();
    } catch {
      // already closed
    }
    if (savedPath === undefined) delete process.env.PATH;
    else process.env.PATH = savedPath;
    if (savedFrontEnd !== undefined)
      process.env.CSTAN_FRONT_END = savedFrontEnd;
    removeTempDir(scratch);
  }
}

type Hooks = ReturnType<typeof installHooks>;

async function perform(
  step: Step,
  launcher: Launcher,
  world: World,
  core: ControllerCore,
  hooks: Hooks,
  reopen: () => void,
): Promise<unknown> {
  const text = (key: string): string => String(step[key]);
  switch (step.op) {
    case "launch_pm":
      return hooks.run(() => launcher.launchPm());
    case "restart_pm":
      return hooks.run(() => launcher.restartPm());
    case "spawn":
      return hooks.run(() =>
        launcher.spawn(text("role"), (step.options ?? {}) as never),
      );
    case "release":
      return hooks.run(() => launcher.release(text("agent")));
    case "replace":
      return hooks.run(() => launcher.replace(text("agent")));
    case "adopt_all":
      return hooks.run(() => launcher.adoptAll());
    case "observe":
      return hooks.run(() =>
        launcher.observe(text("agent"), Number(step.lines ?? 40)),
      );
    case "rename_branch_for_task":
      return hooks.run(() =>
        launcher.renameBranchForTask(text("agent"), text("task")),
      );
    case "interrupt":
      return hooks.run(() => launcher.interrupt(text("agent")));
    case "status":
      return hooks.run(() => launcher.status());
    case "operator_environment":
      return launcher.operatorEnvironment();
    case "in_flight":
      return launcher.inFlightOperations();
    case "reopen":
      reopen();
      return null;
    case "git": {
      const cwd =
        step.cwd === undefined || step.cwd === "root"
          ? world.projectRoot
          : text("cwd");
      const out = git(world, cwd, step.args as string[]);
      return { stdout: out.trim() };
    }
    case "write": {
      const file = path.join(text("dir"), text("file"));
      mkdirSync(path.dirname(file), { recursive: true });
      writeFileSync(file, text("text"));
      return null;
    }
    case "report": {
      // An accepted report by the worker, using the token it was started with.
      const agent = text("agent");
      const token = world.tokens.get(agent);
      if (token === undefined) throw new Error(`no token for ${agent}`);
      return hooks.run(() => {
        const row = core
          .agentPanes(OWNER_CREDENTIAL)
          .find((r) => r.agentId === agent)!;
        const { record } = core.recordAgentReport(newContext(core, token), {
          commitSha: text("commit"),
          summary: text("summary"),
          evidence: {
            generation: 1,
            branch: row.branch!,
            baseSha: row.baseSha!,
            commitExists: true,
            branchTip: text("commit"),
            isAncestorOfTip: true,
            isAncestorOfBase: false,
            checkedAt: "2026-10-01T00:00:00.000Z",
          },
        });
        return { state: record.state };
      });
    }
    case "send": {
      // A message from one agent to another, using the token the sender was started with.
      const token = world.tokens.get(text("from"));
      if (token === undefined) throw new Error(`no token for ${text("from")}`);
      return hooks.run(() =>
        core.enqueueMessage(newContext(core, token), {
          recipientAgentId: text("to"),
          body: text("body"),
        }),
      );
    }
    case "stub":
      return stub(step, world);
    default:
      throw new Error(`unknown op ${step.op}`);
  }
}

/** Changes what the stub adapter and the command runners do next. */
function stub(step: Step, world: World): null {
  const adapter = world.adapter;
  const pane = String(step.pane);
  switch (step.do) {
    case "fail": {
      const queue = world.failures.get(String(step.method)) ?? [];
      for (let n = 0; n < Number(step.times ?? 1); n += 1)
        queue.push(step.error as FailureSpec);
      world.failures.set(String(step.method), queue);
      break;
    }
    case "identity":
      adapter.identities.set(pane, {
        terminalId: step.terminalId as string | undefined,
        agent: step.agent as string | undefined,
        project: step.project as string | undefined,
      });
      break;
    case "gone":
      adapter.gonePanes.add(pane);
      break;
    case "forget_registry":
      adapter.entries.clear();
      adapter.agentPanes.clear();
      break;
    case "observation":
      adapter.observation = step.value as "idle" | "blocked" | "working";
      break;
    case "start_status":
      adapter.startStatus = step.value as "started" | "blocked_at_startup";
      break;
    case "dialog_handled":
      adapter.dialogHandled = step.value === true;
      break;
    case "screen":
      adapter.screens.set(pane, String(step.text));
      break;
    case "working":
      adapter.workingPanes.add(pane);
      break;
    case "zoomed":
      adapter.zoomed = step.value === true;
      break;
    case "strays":
      // Panes Herdr shows at a directory; `<worktrees>` stands for the directory of the stub's worktrees.
      adapter.strays.set(
        String(step.directory).replace("<worktrees>", adapter.worktreeBase),
        step.panes as Array<{ paneId: string; workspaceId: string }>,
      );
      break;
    case "layout_size":
      adapter.layoutSize = {
        width: Number(step.width),
        height: Number(step.height),
      };
      break;
    case "setup":
      world.setupOutcomes.push(...(step.outcomes as SetupOutcome[]));
      break;
    case "teardown":
      world.teardownOutcomes.push(...(step.outcomes as SetupOutcome[]));
      break;
    default:
      throw new Error(`unknown stub action ${String(step.do)}`);
  }
  return null;
}

// ------------------------------------------------------------------------------------------------ files

export function sequenceFiles(): string[] {
  return readdirSync(SEQUENCES_DIRECTORY)
    .filter((name) => name.endsWith(".json"))
    .sort();
}

/** Every fixture file by name, as the text the exporter writes. */
export async function exportFixtures(): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  for (const file of sequenceFiles()) {
    const group = file.slice(0, -".json".length);
    const source = JSON.parse(
      readFileSync(path.join(SEQUENCES_DIRECTORY, file), "utf8"),
    ) as { sequences: SequenceSource[] };
    const names = new Set<string>();
    const sequences: Json[] = [];
    for (const sequence of source.sequences) {
      if (names.has(sequence.name))
        throw new Error(`${file}: sequence ${sequence.name} is repeated`);
      names.add(sequence.name);
      sequences.push(await runSequence(sequence));
    }
    out.set(`${group}.json`, packedText({ format: 2, group, sequences }));
  }
  return out;
}

if (import.meta.filename === process.argv[1]) {
  const outIndex = process.argv.indexOf("--out");
  const directory =
    outIndex >= 0 ? String(process.argv[outIndex + 1]) : PARITY_DIRECTORY;
  mkdirSync(directory, { recursive: true });
  const files = await exportFixtures();
  for (const [name, content] of files)
    writeFileSync(path.join(directory, name), content);
  process.stdout.write(`wrote ${files.size} fixtures to ${directory}\n`);
}

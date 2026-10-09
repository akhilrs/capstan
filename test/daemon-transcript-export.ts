/**
 * Writes the wire transcripts the Rust daemon (rust/crates/daemon) is tested against:
 * rust/crates/daemon/tests/transcripts/<group>.json for every test/daemon-scenarios/<group>.json. A scenario is run against the
 * real Node daemon (`node --import dist/test/daemon-transcript-hooks.js dist/src/cli.js daemon`, `CAPSTAN_LAUNCH=off`) in a
 * scratch project under the temp directory, with seeded time and randomness (test/daemon-transcript-hooks.ts, which reuses
 * test/kernel-parity-hooks.ts), and what happened is recorded: every request frame, the response line, the daemon's log
 * lines and the ledger's tables. The Rust replay (rust/crates/daemon/tests/replay.rs) runs the same requests through
 * `capstan_daemon::handlers` and compares. The scenario language is in test/daemon-scenarios/README.md.
 *
 *   npm run build && node dist/test/daemon-transcript-export.js [--out <dir>]
 *
 * writes the repository's committed files, or the same tree under <dir>. test/daemon-transcript.test.ts fails while the
 * committed files differ from a fresh export.
 *
 * File format (2), as the kernel's: `baseline` is the full ledger (every table with rows) at the end of the group's smallest
 * scenario; every table dump is recorded as `tablesDiff` against it; `dict` holds the strings of 16 or more characters and
 * the table rows that occur more than once, each occurrence being `{"$d": index}`. One scenario per line.
 */
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import net from "node:net";
import path from "node:path";
import { ControllerCore } from "../src/controller/core.js";
import { openDatabase } from "../src/controller/database.js";
import { PLACEHOLDER_INPUTS } from "../src/daemon.js";
import {
  authored,
  compactSequence,
  dumpTables,
  packedText,
  sortedKeys,
  stable,
  type Tables,
} from "./kernel-parity-export.js";
import { installHooks, SeededStream } from "./kernel-parity-hooks.js";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const SCENARIOS_DIRECTORY = path.join(root, "test", "daemon-scenarios");
export const TRANSCRIPTS_LAYOUT = "rust/crates/daemon/tests/transcripts";
const CLI = path.join(root, "dist", "src", "cli.js");
const HOOKS = path.join(root, "dist", "test", "daemon-transcript-hooks.js");
/** The longest frame the daemon reads (src/daemon.ts MAX_FRAME_BYTES); a longer one is a connection-level matter. */
const MAX_FRAME_BYTES = 65_536;
/** A frame this short is recorded as its text, a longer one as the generator that made it. */
const MAX_RECORDED_FRAME = 4_096;
// The control socket path must fit sockaddr_un (about 104 bytes), so scratch directories have short names.
const STEP_TIMEOUT_MS = 20_000;

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

// ------------------------------------------------------------------------------------------------ scenarios

type FrameSpec =
  | string
  | { base64: string }
  | { json: unknown }
  | {
      repeat: {
        prefix?: string;
        text: string;
        times: number;
        suffix?: string;
      };
    };

interface RequestSource {
  /** `operator`, `agent:<id>`, `token:<literal>` or `none` (default `operator`). */
  as?: string;
  label?: string;
  command?: string;
  args?: unknown[];
  frame?: FrameSpec;
  newline?: boolean;
  concurrent?: boolean;
  hold_ms?: number;
  /** Open this many connections that send nothing and stay open until the request is answered. */
  idle_connections?: number;
  dump?: boolean;
}

interface SetupStep {
  op: string;
  context?: Record<string, unknown>;
  args?: unknown[];
  as?: string;
}

interface RepoSource {
  /** The files of the first commit on `main`. */
  files: Record<string, string>;
  /** Further branches: `from` (default `main`), the files they change and the commit message. */
  branches?: Record<
    string,
    { from?: string; files: Record<string, string>; message?: string }
  >;
}

interface ScenarioSource {
  name: string;
  seed?: string;
  /** `socket` replays the whole scenario against the real daemon over its socket. */
  mode?: "socket";
  setup?: SetupStep[];
  repo?: RepoSource;
  /** The text of the project's `capstan.toml` (mode 0600), written before the ledger is touched. */
  config?: string;
  requires?: string[];
  requests: RequestSource[];
}

// ------------------------------------------------------------------------------------------------ the scratch project

function sha256Hex(text: string): string {
  return createHash("sha256").update(text).digest("hex");
}

interface ProjectIdentity {
  readonly projectId: string;
  readonly name: string;
  readonly credential: string;
}

function identityOf(seed: string): ProjectIdentity {
  return {
    projectId: `p${sha256Hex(`project:${seed}`).slice(0, 32)}`,
    name: "Transcript Project",
    credential: `operator-${sha256Hex(`credential:${seed}`)}`,
  };
}

function writeProject(directory: string, identity: ProjectIdentity): string {
  const capstan = path.join(directory, ".capstan");
  const state = path.join(capstan, "state");
  mkdirSync(capstan, { mode: 0o700 });
  chmodSync(capstan, 0o700);
  writeFileSync(
    path.join(capstan, "project.json"),
    `${JSON.stringify(
      {
        schemaVersion: 1,
        projectId: identity.projectId,
        name: identity.name,
        stateDirectory: state,
        maxSlices: 4,
        maxRunMs: 3_600_000,
        maxDispatches: 16,
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  writeFileSync(
    path.join(capstan, "operator.key"),
    `${identity.credential}\n`,
    {
      mode: 0o600,
    },
  );
  mkdirSync(state, { mode: 0o700 });
  chmodSync(state, 0o700);
  return state;
}

// ------------------------------------------------------------------------------------------------ the scratch repository

/** Fixed author, committer and dates, so a commit has the same id in every run and in the Rust replay. */
const GIT_ENVIRONMENT = {
  PATH: process.env.PATH ?? "/usr/bin:/bin",
  LC_ALL: "C",
  HOME: "/nonexistent",
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_CONFIG_GLOBAL: "/dev/null",
  GIT_AUTHOR_NAME: "Transcript",
  GIT_AUTHOR_EMAIL: "transcript@example.invalid",
  GIT_COMMITTER_NAME: "Transcript",
  GIT_COMMITTER_EMAIL: "transcript@example.invalid",
  GIT_AUTHOR_DATE: "2026-01-01T00:00:00+00:00",
  GIT_COMMITTER_DATE: "2026-01-01T00:00:00+00:00",
};

function git(directory: string, args: string[]): string {
  const result = spawnSync("git", args, {
    cwd: directory,
    env: GIT_ENVIRONMENT,
    encoding: "utf8",
  });
  if (result.status !== 0)
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  return result.stdout.trim();
}

function writeFiles(directory: string, files: Record<string, string>): void {
  for (const [name, content] of Object.entries(files)) {
    const file = path.join(directory, name);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, content);
  }
}

/** Builds the scenario's repository in the project directory; returns the tip of every branch (main first). */
function buildRepo(
  directory: string,
  repo: RepoSource,
): Record<string, string> {
  git(directory, ["init", "--quiet", "--initial-branch=main"]);
  writeFileSync(path.join(directory, ".git", "info", "exclude"), ".capstan/\n");
  writeFiles(directory, repo.files);
  git(directory, ["add", "--all"]);
  git(directory, ["commit", "--quiet", "-m", "chore: initial commit"]);
  const heads: Record<string, string> = {
    main: git(directory, ["rev-parse", "main"]),
  };
  for (const [branch, spec] of Object.entries(repo.branches ?? {})) {
    git(directory, ["checkout", "--quiet", "-b", branch, spec.from ?? "main"]);
    writeFiles(directory, spec.files);
    git(directory, ["add", "--all"]);
    git(directory, [
      "commit",
      "--quiet",
      "-m",
      spec.message ?? `feat: change on ${branch}`,
    ]);
    heads[branch] = git(directory, ["rev-parse", branch]);
  }
  git(directory, ["checkout", "--quiet", "main"]);
  return heads;
}

// ------------------------------------------------------------------------------------------------ setup steps

interface SetupContext {
  readonly bindings: Map<string, unknown>;
  core: ControllerCore | undefined;
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

function resolveSetup(value: unknown, context: SetupContext): unknown {
  if (typeof value === "string") {
    const match = /^\$([A-Za-z_][\w-]*)((?:\.[\w-]+)*)$/.exec(value);
    if (match === null) return value;
    const name = match[1]!;
    if (name === "version" || name === "revision") {
      const core = context.core;
      if (core === undefined) throw new Error("no controller is open");
      return name === "version" ? core.stateVersion : core.inputRevision;
    }
    if (!context.bindings.has(name)) return value;
    const parts = match[2] === "" ? [] : match[2]!.slice(1).split(".");
    return lookup(context.bindings.get(name), parts, value);
  }
  if (Array.isArray(value))
    return value.map((entry) => resolveSetup(entry, context));
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [
        key,
        resolveSetup(entry, context),
      ]),
    );
  return value;
}

function errorOf(error: unknown): { name: string; message: string } {
  const e = error as { name?: unknown; message?: unknown };
  return {
    name: typeof e?.name === "string" ? e.name : "Error",
    message: typeof e?.message === "string" ? e.message : String(error),
  };
}

/**
 * Applies the `setup` steps to the scratch ledger before the daemon opens it: the kernel-sequences language (README of
 * test/kernel-sequences), with one controller (`main`) opened on the daemon's project and closed after the last step.
 */
async function runSetup(
  steps: SetupStep[],
  seed: string,
  stateDirectory: string,
  directory: string,
  identity: ProjectIdentity,
  bindings: Map<string, unknown>,
): Promise<Json[]> {
  const hooks = installHooks(seed);
  const context: SetupContext = { bindings, core: undefined };
  bindings.set("owner", { credential: identity.credential });
  bindings.set("internal", {
    credential: new SeededStream(seed).bytes(32).toString("base64url"),
  });
  const records: Json[] = [];
  try {
    context.core = await hooks.run(() =>
      ControllerCore.open({
        stateDirectory,
        project: {
          projectId: identity.projectId,
          name: identity.name,
          ownerCredential: identity.credential,
          initialInputs: PLACEHOLDER_INPUTS,
        },
        workspaceRoot: directory,
      }),
    );
    for (const [index, step] of steps.entries()) {
      const core = context.core;
      const record: { [key: string]: Json } = { op: step.op };
      let result: unknown = null;
      let failure: { name: string; message: string } | undefined;
      try {
        const args: unknown[] = [];
        if (step.context !== undefined) {
          const resolved = resolveSetup(step.context, context) as Record<
            string,
            unknown
          >;
          const made = {
            credential: resolved.credential,
            requestId: resolved.requestId ?? `req-${index}`,
            idempotencyKey: resolved.idempotencyKey ?? `idem-${index}`,
            expectedVersion: resolved.expectedVersion ?? core.stateVersion,
            inputRevision: resolved.inputRevision ?? core.inputRevision,
          };
          record.context = stable(made);
          args.push(made);
        }
        const rest = resolveSetup(step.args ?? [], context) as unknown[];
        args.push(...rest);
        record.args = authored(rest);
        const member = (core as unknown as Record<string, unknown>)[step.op];
        const raw: unknown = hooks.run(() =>
          typeof member === "function"
            ? (member as (...a: unknown[]) => unknown).apply(core, args)
            : member,
        );
        result = stable(
          JSON.parse(JSON.stringify(raw === undefined ? null : raw)),
        );
        if (step.as !== undefined) bindings.set(step.as, result);
      } catch (error) {
        failure = errorOf(error);
      }
      if (failure === undefined) record.result = stable(result);
      else {
        record.error = failure.name;
        record.message = failure.message;
      }
      if (step.as !== undefined) record.as = step.as;
      records.push(sortedKeys(record));
    }
  } finally {
    context.core?.close();
    hooks.restore();
  }
  return records;
}

// ------------------------------------------------------------------------------------------------ the daemon

interface Daemon {
  readonly child: ChildProcess;
  /** Every complete line of standard output, in order. */
  readonly lines: string[];
  /** How many `request_start` lines of the hooks have been seen. */
  starts: number;
  readonly stderr: string[];
  exit: Promise<number>;
}

function pollUntil(
  condition: () => boolean,
  what: string,
  timeoutMs = STEP_TIMEOUT_MS,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const check = (): void => {
      if (condition()) resolve();
      else if (Date.now() > deadline)
        reject(new Error(`timed out waiting for ${what}`));
      else setTimeout(check, 5);
    };
    check();
  });
}

async function startDaemon(directory: string, seed: string): Promise<Daemon> {
  const child = spawn(process.execPath, ["--import", HOOKS, CLI, "daemon"], {
    cwd: directory,
    env: {
      PATH: process.env.PATH ?? "/usr/bin:/bin",
      LANG: "C",
      TMPDIR: process.env.TMPDIR ?? "/tmp",
      CAPSTAN_LAUNCH: "off",
      CAPSTAN_TRANSCRIPT_SEED: seed,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const daemon: Daemon = {
    child,
    lines: [],
    starts: 0,
    stderr: [],
    exit: new Promise((resolve) =>
      child.once("exit", (code, signal) => resolve(code ?? (signal ? -1 : 0))),
    ),
  };
  let partial = "";
  child.stdout!.setEncoding("utf8");
  child.stdout!.on("data", (chunk: string) => {
    partial += chunk;
    for (;;) {
      const newline = partial.indexOf("\n");
      if (newline < 0) break;
      const line = partial.slice(0, newline);
      partial = partial.slice(newline + 1);
      if (line === '{"event":"request_start"}') daemon.starts += 1;
      else daemon.lines.push(line);
    }
  });
  child.stderr!.setEncoding("utf8");
  child.stderr!.on("data", (chunk: string) => daemon.stderr.push(chunk));
  let exited = false;
  void daemon.exit.then(() => {
    exited = true;
  });
  await pollUntil(
    () =>
      exited || daemon.lines.some((line) => line.includes('"event":"ready"')),
    "the daemon to be ready",
  );
  if (exited)
    throw new Error(
      `the daemon exited while starting: ${daemon.stderr.join("")}${daemon.lines.join("\n")}`,
    );
  return daemon;
}

/** Log entries are the lines that carry a `command` member; announcements (`event`) are not. */
function entryLines(daemon: Daemon): string[] {
  return daemon.lines.filter((line) => line.includes('"command"'));
}

// ------------------------------------------------------------------------------------------------ requests

interface Frame {
  readonly bytes: Buffer;
  /** What the transcript records: the text, or the generator for a long or non-text frame. */
  readonly record: Json;
}

function credentialOf(
  as: string,
  identity: ProjectIdentity,
  agents: Map<string, string>,
): string | undefined {
  if (as === "operator") return identity.credential;
  if (as === "none") return undefined;
  if (as.startsWith("token:")) return as.slice("token:".length);
  if (as.startsWith("agent:")) {
    const credential = agents.get(as.slice("agent:".length));
    if (credential === undefined)
      throw new Error(
        `${as}: no setup step created that agent with a credential`,
      );
    return credential;
  }
  throw new Error(`unknown "as": ${as}`);
}

function substitute(
  value: unknown,
  replace: (text: string) => string,
): unknown {
  if (typeof value === "string") return replace(value);
  if (Array.isArray(value))
    return value.map((entry) => substitute(entry, replace));
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [
        key,
        substitute(entry, replace),
      ]),
    );
  return value;
}

function buildFrame(
  source: RequestSource,
  credential: string | undefined,
  heads: Record<string, string>,
  bindings: Map<string, unknown>,
): Frame {
  const shas = (text: string): string =>
    text.replace(/\$sha:([A-Za-z0-9._/-]+)/g, (_, branch: string) => {
      const sha = heads[branch];
      if (sha === undefined) throw new Error(`no branch ${branch} in the repo`);
      return sha;
    });
  const withCredential = (text: string): string => {
    if (text === "$credential") {
      if (credential === undefined)
        throw new Error('"$credential" needs a credential ("as")');
      return credential;
    }
    // A whole string `$name.path` is a field of a result bound by a setup step.
    const reference = /^\$([A-Za-z_][\w-]*)((?:\.[\w-]+)*)$/.exec(text);
    if (reference !== null && bindings.has(reference[1]!)) {
      const parts =
        reference[2] === "" ? [] : reference[2]!.slice(1).split(".");
      return String(lookup(bindings.get(reference[1]!), parts, text));
    }
    return shas(text);
  };
  let spec = source.frame;
  if (spec === undefined) {
    if (source.command === undefined)
      throw new Error("a request needs a command or a frame");
    spec = {
      json: {
        v: 1,
        ...(credential === undefined ? {} : { credential: "$credential" }),
        command: source.command,
        args: source.args ?? [],
      },
    };
  }
  let bytes: Buffer;
  let record: Json;
  if (typeof spec === "string" || "json" in spec) {
    const text =
      typeof spec === "string"
        ? shas(spec)
        : JSON.stringify(substitute(spec.json, withCredential));
    bytes = Buffer.from(text, "utf8");
    // A frame that is short text is recorded as that text; a long one as its bytes.
    record =
      bytes.length <= MAX_RECORDED_FRAME
        ? text
        : { base64: bytes.toString("base64") };
  } else if ("base64" in spec) {
    bytes = Buffer.from(spec.base64, "base64");
    record = { base64: spec.base64 };
  } else {
    const { prefix = "", text, times, suffix = "" } = spec.repeat;
    bytes = Buffer.from(`${prefix}${text.repeat(times)}${suffix}`, "utf8");
    record = { repeat: { prefix, text, times, suffix } };
  }
  return { bytes, record };
}

interface Exchange {
  response: string | null;
  closed: boolean;
}

function exchange(
  socketPath: string,
  bytes: Buffer,
  holdMs: number | undefined,
): Promise<Exchange> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let closed = false;
    let settled = false;
    const socket = net.connect(socketPath);
    const finish = (): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      const data = Buffer.concat(chunks);
      resolve({
        response:
          data.length === 0 ? null : data.toString("utf8").replace(/\n$/, ""),
        closed,
      });
    };
    const timer = setTimeout(() => {
      if (holdMs !== undefined) {
        socket.destroy();
        finish();
      } else {
        socket.destroy();
        if (!settled) reject(new Error("the request was not answered"));
      }
    }, holdMs ?? STEP_TIMEOUT_MS);
    socket.on("data", (chunk: Buffer) => chunks.push(chunk));
    socket.on("error", () => undefined);
    socket.on("close", () => {
      closed = true;
      finish();
    });
    socket.on("connect", () => {
      socket.write(bytes, () => undefined);
    });
  });
}

/**
 * Connections that send nothing. Each is connected before the next opens and the server accepts in order, so the request
 * that follows is the one beyond the connection limit.
 */
async function openIdle(
  socketPath: string,
  count: number,
): Promise<net.Socket[]> {
  const sockets: net.Socket[] = [];
  for (let i = 0; i < count; i += 1) {
    const socket = net.connect(socketPath);
    socket.on("error", () => undefined);
    await new Promise<void>((resolve) => socket.once("connect", resolve));
    sockets.push(socket);
  }
  await new Promise((resolve) => setTimeout(resolve, 200));
  return sockets;
}

/** What a transcript redacts: the scratch project's path, and the daemon's process id. */
function redactor(directory: string): (text: string) => string {
  const paths = new Set([directory, realpathSync(directory)]);
  return (text) => {
    let out = text;
    for (const p of paths) out = out.split(p).join("<project>");
    return out.replace(/"pid":\d+/g, '"pid":0');
  };
}

function totalRows(tables: Tables): number {
  return Object.values(tables).reduce((sum, t) => sum + t.rows.length, 0);
}

async function runScenario(
  source: ScenarioSource,
  group: string,
): Promise<{ record: Json; tables: Tables }> {
  const seed = source.seed ?? group;
  const identity = identityOf(`${group}/${source.name}/${seed}`);
  const directory = tempDir("cdt-");
  const redact = redactor(directory);
  let daemon: Daemon | undefined;
  try {
    const stateDirectory = writeProject(directory, identity);
    if (source.config !== undefined)
      writeFileSync(path.join(directory, "capstan.toml"), source.config, {
        mode: 0o600,
      });
    const heads =
      source.repo === undefined ? undefined : buildRepo(directory, source.repo);
    // The ledger's migration timestamps come from the system clock in both implementations; migrating first keeps them
    // out of the seeded clock.
    const database = path.join(stateDirectory, "controller.sqlite");
    (await openDatabase(database)).close();
    const bindings = new Map<string, unknown>();
    const setup =
      source.setup === undefined
        ? undefined
        : await runSetup(
            source.setup,
            `${seed}/setup`,
            stateDirectory,
            directory,
            identity,
            bindings,
          );
    const agents = new Map<string, string>();
    for (const value of bindings.values()) {
      const entry = value as Record<string, unknown> | null;
      if (
        entry !== null &&
        typeof entry === "object" &&
        typeof entry.credential === "string" &&
        typeof entry.actorId === "string"
      )
        agents.set(String(entry.actorId), entry.credential);
    }
    for (const step of setup ?? []) {
      const result = (step as { result?: Record<string, unknown> }).result;
      if (
        result !== undefined &&
        result !== null &&
        typeof result.agentId === "string" &&
        typeof result.actorId === "string"
      ) {
        const credential = agents.get(result.actorId);
        if (credential !== undefined) agents.set(result.agentId, credential);
      }
    }
    daemon = await startDaemon(directory, `${seed}/daemon`);
    const socketPath = path.join(stateDirectory, "control.sock");
    const steps: Json[] = [];
    const pending: Promise<void>[] = [];
    let finished = 0;
    let expectedEntries = entryLines(daemon).length;
    let previous: { startsAtSend: number; done: boolean } | undefined;
    for (const [index, request] of source.requests.entries()) {
      const as = request.as ?? "operator";
      const credential = credentialOf(as, identity, agents);
      const frame = buildFrame(request, credential, heads ?? {}, bindings);
      const withNewline = request.newline !== false;
      const payload = withNewline
        ? Buffer.concat([frame.bytes, Buffer.from("\n")])
        : frame.bytes;
      const layer =
        frame.bytes.length > MAX_FRAME_BYTES ||
        !withNewline ||
        request.hold_ms !== undefined ||
        request.idle_connections !== undefined
          ? "socket"
          : "dispatch";
      if (request.concurrent === true && previous !== undefined) {
        const waitFor = previous;
        await pollUntil(
          () => waitFor.done || daemon!.starts > waitFor.startsAtSend,
          `request ${index - 1} to start`,
        );
      } else {
        await Promise.all(pending);
        await pollUntil(
          () => entryLines(daemon!).length >= expectedEntries,
          `the log entry of request ${index - 1}`,
        );
      }
      if (layer === "dispatch") expectedEntries += 1;
      const mine = { startsAtSend: daemon.starts, done: false };
      previous = mine;
      const record: { [key: string]: Json } = {
        as,
        frame: frame.record,
        layer,
        newline: withNewline,
      };
      if (request.label !== undefined) record.label = request.label;
      if (request.command !== undefined) record.command = request.command;
      if (request.concurrent === true) record.concurrent = true;
      if (request.hold_ms !== undefined) record.hold_ms = request.hold_ms;
      if (request.idle_connections !== undefined)
        record.idle_connections = request.idle_connections;
      steps.push(record);
      const idle =
        request.idle_connections === undefined
          ? []
          : await openIdle(socketPath, request.idle_connections);
      pending.push(
        exchange(socketPath, payload, request.hold_ms).then((outcome) => {
          for (const socket of idle) socket.destroy();
          mine.done = true;
          finished += 1;
          record.response =
            outcome.response === null ? null : redact(outcome.response);
          record.closed = outcome.closed;
          record.finished = finished;
        }),
      );
      if (request.dump === true) {
        await Promise.all(pending);
        await pollUntil(
          () => entryLines(daemon!).length >= expectedEntries,
          `the log entry of request ${index}`,
        );
        record.tables = dumpTables(database);
      }
    }
    await Promise.all(pending);
    await pollUntil(
      () => entryLines(daemon!).length >= expectedEntries,
      "the last log entry",
    );
    const tables = dumpTables(database) as unknown as Tables;
    daemon.child.kill("SIGTERM");
    const exit = await daemon.exit;
    const log = daemon.lines
      .filter((line) => line.includes('"command"'))
      .map(redact);
    const sequence: { [key: string]: Json } = {
      name: source.name,
      seed,
      ...(source.mode === undefined ? {} : { mode: source.mode }),
      requires: source.requires ?? [],
      project: {
        projectId: identity.projectId,
        name: identity.name,
        credential: identity.credential,
      },
      ...(setup === undefined ? {} : { setup }),
      ...(source.config === undefined ? {} : { config: source.config }),
      ...(source.repo === undefined
        ? {}
        : { repo: { ...(stable(source.repo) as object), heads: heads! } }),
      steps: steps.map((step) => sortedKeys(step as { [key: string]: Json })),
      log,
      exit,
      tables: tables as never,
    };
    return { record: sortedKeys(sequence), tables };
  } finally {
    if (daemon !== undefined && daemon.child.exitCode === null) {
      daemon.child.kill("SIGKILL");
      await daemon.exit;
    }
    removeTempDir(directory);
  }
}

// ------------------------------------------------------------------------------------------------ the export

export function scenarioFiles(): string[] {
  return readdirSync(SCENARIOS_DIRECTORY)
    .filter((name) => name.endsWith(".json"))
    .sort();
}

async function groupText(group: string): Promise<string> {
  const source = JSON.parse(
    readFileSync(path.join(SCENARIOS_DIRECTORY, `${group}.json`), "utf8"),
  ) as { scenarios: ScenarioSource[] };
  const names = new Set<string>();
  const runs: { record: Json; tables: Tables }[] = [];
  for (const scenario of source.scenarios) {
    if (names.has(scenario.name))
      throw new Error(`${group}.json: scenario ${scenario.name} is repeated`);
    names.add(scenario.name);
    runs.push(await runScenario(scenario, group));
  }
  // The baseline is the ledger the group's smallest scenario ends with.
  const smallest = runs.reduce((best, run) =>
    totalRows(run.tables) < totalRows(best.tables) ? run : best,
  );
  const baseline = smallest.tables;
  const scenarios = runs.map((run) =>
    compactSequence(run.record, baseline as Tables),
  );
  return packedText(
    {
      format: 2,
      group,
      baseline: baseline as never,
      scenarios: scenarios.map((scenario) =>
        sortedKeys(scenario as { [key: string]: Json }),
      ),
    },
    { keepOrder: true },
  );
}

/** Every exported file by repository-relative path, as the text the exporter writes. */
export async function exportFiles(): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  for (const file of scenarioFiles()) {
    const group = file.slice(0, -".json".length);
    files.set(`${TRANSCRIPTS_LAYOUT}/${group}.json`, await groupText(group));
  }
  return files;
}

if (import.meta.filename === process.argv[1]) {
  const outIndex = process.argv.indexOf("--out");
  const base =
    outIndex === -1 ? root : path.resolve(process.argv[outIndex + 1] ?? "");
  const files = await exportFiles();
  for (const [relative, content] of files) {
    const file = path.join(base, relative);
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, content);
  }
  process.stdout.write(`wrote ${files.size} files under ${base}\n`);
}

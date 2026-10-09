/**
 * Writes the fixtures the Rust loops (rust/crates/daemon/src/loops) are tested against:
 * rust/crates/daemon/tests/loops-parity/<group>.json for every test/loops-sequences/<group>.json. The real Node loops
 * (src/driver.ts DeliveryDriver, src/supervision.ts, src/reports.ts startReportRelay, with src/notifier.ts and
 * src/pm-mail.ts) run in process against a recording stub of the Herdr adapter and a seeded ControllerCore, one scripted
 * tick at a time, and what happened is recorded: every adapter call, log event, notification line, the calls made on the
 * core (by method, counted by a proxy: no edit of src/), the driver's snapshot and the ledger's table changes. The Rust
 * replay (rust/crates/daemon/tests/loops.rs) runs the same ticks against `StubDriverAdapter` and compares everything.
 * The sequence language is in test/loops-sequences/README.md.
 *
 *   npm run build && node dist/test/loops-parity-export.js [--out <dir>]
 *
 * writes the repository's committed files, or the same tree under <dir>. test/loops-parity.test.ts fails while the
 * committed files differ from a fresh export.
 *
 * The controller reads the clock and randomness through test/kernel-parity-hooks.ts, seeded per sequence, as the kernel
 * export does; the loops' own time is scripted (a tick's `at`), except the report relay, which has no clock option and
 * reads Date.now() (one reading of the seeded clock per use, as the Rust relay reads the kernel's clock).
 */
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { ControllerCore } from "../src/controller/core.js";
import {
  openDatabase,
  resolveDatabasePath,
} from "../src/controller/database.js";
import { PLACEHOLDER_INPUTS } from "../src/daemon.js";
import { DeliveryDriver, type DriverAdapter } from "../src/driver.js";
import {
  AgentPaneMismatch,
  ClearFailed,
  DeferralNotElapsed,
  InputUnreadable,
  InvalidArgumentError,
  NotIdle,
  PhaseError,
  SendAfterRecordError,
  UnknownPaneError,
} from "../src/herdr/adapter.js";
import { LauncherError } from "../src/launcher/shared.js";
import type { ProcessActivityProbe } from "../src/herdr/process-activity.js";
import { HerdrError } from "../src/herdr/runner.js";
import { newContext } from "../src/context.js";
import {
  recoverIntegrations,
  type IntegrationGit,
} from "../src/integration.js";
import { createNotifier } from "../src/notifier.js";
import { pmMailSummary } from "../src/pm-mail.js";
import { startReportRelay } from "../src/reports.js";
import { startSupervision } from "../src/supervision.js";
import {
  authored,
  diffTables,
  dumpTables,
  stable,
  type Tables,
} from "./kernel-parity-export.js";
import {
  installHooks,
  SEEDED_EPOCH_MS,
  SeededStream,
} from "./kernel-parity-hooks.js";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const SEQUENCES_DIRECTORY = path.join(root, "test", "loops-sequences");
export const PARITY_DIRECTORY = "rust/crates/daemon/tests/loops-parity";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

// ------------------------------------------------------------------------------------------------ sources

interface StepSource {
  readonly op: string;
  readonly context?: Record<string, unknown>;
  readonly args?: unknown[];
  readonly as?: string;
}

/** What the stub adapter answers; each section is merged key by key into the one before (null removes a key). */
interface AdapterPatch {
  /** agentId -> paneId. The pane's registry entry names the same agent unless `entries` says otherwise. */
  readonly panes?: Record<string, string | null>;
  /** paneId -> agent the registry entry names (null: no entry). */
  readonly entries?: Record<string, string | null>;
  /** agentId -> a state, or `{ "error": "<spec>" }`. */
  readonly observe?: Record<string, unknown>;
  /** paneId -> "sent" | { deferred, detail?, blocker? } | { error } | { hookThenError }. */
  readonly send?: Record<string, unknown>;
  /** paneId -> "sent" | "pm_not_idle" | "input_not_empty" | { error } | { hookThenError }. */
  readonly wake?: Record<string, unknown>;
  /** paneId -> { cleared, text?, keys? } | { error }. */
  readonly clear?: Record<string, unknown>;
  /** "ok" | { error } for the notification of the Herdr channel. */
  readonly notify?: unknown;
  /** paneId -> { processes } | { error }. */
  readonly probe?: Record<string, unknown>;
  /** "ok" | { error } for the launcher (supervision only): spawn and release. */
  readonly spawn?: unknown;
  readonly release?: unknown;
  /** (recover only) git: `tips` branch -> sha | null | { error }, `delete` branch -> bool | { error }, `covered` -> reports | { error }. */
  readonly git?: Record<string, unknown>;
}

interface TickSource {
  /** Milliseconds after the seeded epoch; default: the previous tick's plus the sequence's `tickMs`. */
  readonly at?: number;
  readonly steps?: StepSource[];
  readonly adapter?: AdapterPatch;
  /** Record the table changes of this tick (the default is yes). */
  readonly dump?: boolean;
  /** Run this tick that many times in a row (default 1), with `at` advancing by `tickMs` each time. */
  readonly repeat?: number;
}

interface SequenceSource {
  readonly name: string;
  readonly seed?: string;
  /** Which loop runs: the delivery driver, the supervision tick or the report relay. */
  readonly loop: "driver" | "supervision" | "relay" | "recover";
  /** "team" expands to a PM, a developer, a verifier and a supervisor registered as pm-1, dev-1, ver-1 and sup-1. */
  readonly preset?: "team";
  readonly config?: {
    readonly timers?: Record<string, number>;
    readonly pmStaleSeconds?: number;
    readonly tickMs?: number;
    readonly channels?: { herdr: boolean; fallback: boolean };
    readonly supervision?: { enabled: boolean; checkSeconds: number };
    readonly supervisorRole?: string | null;
    readonly findingCheckSeconds?: number | null;
  };
  /** The agent whose open mail is summarised with pmMailSummary after each tick. */
  readonly pmMail?: string;
  readonly setup?: StepSource[];
  readonly ticks: TickSource[];
}

const DEFAULT_TIMERS = {
  maxDeferralSeconds: 3600,
  maxBusyDeferralSeconds: 3600,
  pmAckTimeoutSeconds: 3600,
  pmNotifyAfterSeconds: 3600,
  notifyIntervalSeconds: 3600,
  stallAfterSeconds: 3600,
  workerAckTimeoutSeconds: 3600,
  pmWakeAfterSeconds: 3600,
  pmWakeIntervalSeconds: 3600,
};

const TEAM: StepSource[] = [
  {
    op: "syncRoleDefinitions",
    context: { credential: "$owner.credential" },
    args: [
      [
        ["pm", "PM"],
        ["developer", "Developer"],
        ["verifier", "Verifier"],
        ["supervisor", "Supervisor"],
      ].map(([name, kind]) => ({
        name,
        kind,
        host: "claude",
        configHash: "a".repeat(64),
      })),
    ],
  },
  ...(
    [
      ["pm", "PM", "pm-1"],
      ["dev", "Developer", "dev-1"],
      ["ver", "Verifier", "ver-1"],
      ["sup", "Supervisor", "sup-1"],
    ] as const
  ).flatMap(([short, role, agentId]): StepSource[] => [
    {
      op: "createSeat",
      context: { credential: "$owner.credential" },
      args: [{ seatId: `seat-${short}`, name: `seat-${short}`, role }],
    },
    {
      op: "createActor",
      context: { credential: "$owner.credential" },
      args: [{ displayName: short, role, seatId: `seat-${short}` }],
      as: short,
    },
    {
      op: "registerAgent",
      context: { credential: "$owner.credential" },
      args: [
        {
          agentId,
          roleName: role === "PM" ? "pm" : role.toLowerCase(),
          seatId: `seat-${short}`,
          actorId: `$${short}.actorId`,
        },
      ],
    },
  ]),
];

// ------------------------------------------------------------------------------------------------ steps

interface StepRunner {
  readonly core: ControllerCore;
  readonly bindings: Map<string, unknown>;
  readonly hooks: ReturnType<typeof installHooks>;
  next: number;
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

function resolveReferences(value: unknown, runner: StepRunner): unknown {
  if (typeof value === "string") {
    const match = /^\$([A-Za-z_][\w-]*)((?:\.[\w-]+)*)$/.exec(value);
    if (match === null) return value;
    const name = match[1]!;
    if (name === "version" || name === "revision")
      return name === "version"
        ? runner.core.stateVersion
        : runner.core.inputRevision;
    if (!runner.bindings.has(name)) return value;
    const parts = match[2] === "" ? [] : match[2]!.slice(1).split(".");
    return lookup(runner.bindings.get(name), parts, value);
  }
  if (Array.isArray(value))
    return value.map((entry) => resolveReferences(entry, runner));
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [
        key,
        resolveReferences(entry, runner),
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

/** Runs a step on the core (the kernel-sequences language) and records it with its arguments resolved. */
function runStep(runner: StepRunner, step: StepSource): Json {
  const index = runner.next++;
  const record: { [key: string]: Json } = { op: step.op };
  let result: unknown = null;
  let failure: { name: string; message: string } | undefined;
  try {
    const args: unknown[] = [];
    if (step.context !== undefined) {
      const resolved = resolveReferences(step.context, runner) as Record<
        string,
        unknown
      >;
      const made = {
        credential: resolved.credential,
        requestId: resolved.requestId ?? `req-${index}`,
        idempotencyKey: resolved.idempotencyKey ?? `idem-${index}`,
        expectedVersion: resolved.expectedVersion ?? runner.core.stateVersion,
        inputRevision: resolved.inputRevision ?? runner.core.inputRevision,
      };
      record.context = stable(made);
      args.push(made);
    }
    const rest = resolveReferences(step.args ?? [], runner) as unknown[];
    args.push(...rest);
    record.args = authored(rest);
    const member = (runner.core as unknown as Record<string, unknown>)[step.op];
    const raw: unknown = runner.hooks.run(() =>
      typeof member === "function"
        ? (member as (...a: unknown[]) => unknown).apply(runner.core, args)
        : member,
    );
    result = stable(JSON.parse(JSON.stringify(raw === undefined ? null : raw)));
    if (step.as !== undefined) runner.bindings.set(step.as, result);
  } catch (error) {
    failure = errorOf(error);
  }
  if (failure === undefined) record.result = stable(result);
  else {
    record.error = failure.name;
    record.message = failure.message;
  }
  return record;
}

// ------------------------------------------------------------------------------------------------ the stub adapter

/**
 * `Class`, `Class:message`, `HerdrError:code[:message]`, `LauncherError:code[:message]` or
 * `InputUnreadable:blocker[:message]` as an error of the
 * adapter's or the runner's classes.
 */
function errorFromSpec(spec: string): Error {
  const [name, arg, ...rest] = spec.split(":");
  const tail = rest.join(":");
  const plain = [arg, ...rest].filter((part) => part !== undefined).join(":");
  const message = (text: string): string =>
    text === "" ? `${name} (scripted)` : text;
  switch (name) {
    case "HerdrError":
      return new HerdrError(arg ?? "failed", message(tail));
    case "LauncherError":
      return new LauncherError(arg ?? "failed", message(tail));
    case "InputUnreadable":
      return new InputUnreadable(
        message(tail),
        (arg ?? "unknown") as "dialog" | "permission_prompt" | "unknown",
      );
    case "AgentPaneMismatch":
      return new AgentPaneMismatch(message(plain));
    case "PhaseError":
      return new PhaseError(message(plain));
    case "UnknownPaneError":
      return new UnknownPaneError(message(plain));
    case "SendAfterRecordError":
      return new SendAfterRecordError(message(plain));
    case "InvalidArgumentError":
      return new InvalidArgumentError(message(plain));
    case "DeferralNotElapsed":
      return new DeferralNotElapsed(message(plain));
    case "NotIdle":
      return new NotIdle(message(plain));
    case "ClearFailed":
      return new ClearFailed(message(plain));
    default:
      return new Error(message(plain));
  }
}

function failureOf(script: unknown): Error | undefined {
  const spec = (script as { error?: unknown } | null)?.error;
  return typeof spec === "string" ? errorFromSpec(spec) : undefined;
}

type Sections = Required<
  Pick<
    AdapterPatch,
    "panes" | "entries" | "observe" | "send" | "wake" | "clear" | "probe"
  >
> & {
  notify: unknown;
  spawn: unknown;
  release: unknown;
  git: Record<string, unknown>;
};

function mergeAdapter(state: Sections, patch: AdapterPatch | undefined): void {
  if (patch === undefined) return;
  for (const section of [
    "panes",
    "entries",
    "observe",
    "send",
    "wake",
    "clear",
    "probe",
  ] as const) {
    const incoming = patch[section] as Record<string, unknown> | undefined;
    if (incoming === undefined) continue;
    const into = state[section] as Record<string, unknown>;
    for (const [key, value] of Object.entries(incoming))
      if (value === null) delete into[key];
      else into[key] = value;
  }
  for (const section of ["notify", "spawn", "release"] as const)
    if (patch[section] !== undefined) state[section] = patch[section];
  if (patch.git !== undefined) state.git = { ...state.git, ...patch.git };
}

function newSections(): Sections {
  return {
    panes: {},
    entries: {},
    observe: {},
    send: {},
    wake: {},
    clear: {},
    probe: {},
    notify: "ok",
    spawn: "ok",
    release: "ok",
    git: {},
  };
}

/** The registry entry a pane has: the agent that owns its pane, unless `entries` names another (or none). */
function entryAgent(state: Sections, paneId: string): string | undefined {
  if (paneId in state.entries) return state.entries[paneId] ?? undefined;
  return Object.entries(state.panes).find(([, pane]) => pane === paneId)?.[0];
}

function stubAdapter(state: Sections, calls: string[]): DriverAdapter {
  return {
    paneForAgent(agentId) {
      calls.push(`paneForAgent ${agentId}`);
      return state.panes[agentId] ?? undefined;
    },
    paneEntry(paneId) {
      calls.push(`paneEntry ${paneId}`);
      const agent = entryAgent(state, paneId);
      return agent === undefined ? undefined : { agent };
    },
    async agentObservation(agentId) {
      calls.push(`agentObservation ${agentId}`);
      const script = state.observe[agentId] ?? "idle";
      const failure = failureOf(script);
      if (failure !== undefined) throw failure;
      return script as "idle" | "working" | "blocked" | "done" | "unknown";
    },
    async guardedSend({ paneId, text, beforeSend }) {
      calls.push(`guardedSend ${paneId} ${JSON.stringify(text)}`);
      const script = state.send[paneId] ?? "sent";
      const failure = failureOf(script);
      if (failure !== undefined) throw failure;
      const after = (script as { hookThenError?: unknown }).hookThenError;
      if (typeof after === "string") {
        await beforeSend();
        throw errorFromSpec(after);
      }
      const deferred = (script as { deferred?: unknown }).deferred;
      if (typeof deferred === "string") {
        const s = script as { detail?: string; blocker?: string };
        return {
          sent: false,
          reason: deferred,
          ...(s.detail === undefined ? {} : { detail: s.detail }),
          ...(s.blocker === undefined ? {} : { blocker: s.blocker }),
        } as never;
      }
      await beforeSend();
      return { sent: true };
    },
    async wakePm({ paneId, text, beforeSend }) {
      calls.push(`wakePm ${paneId} ${JSON.stringify(text)}`);
      const script = state.wake[paneId] ?? "sent";
      const failure = failureOf(script);
      if (failure !== undefined) throw failure;
      if (script === "pm_not_idle" || script === "input_not_empty")
        return { sent: false, reason: script };
      const after = (script as { hookThenError?: unknown }).hookThenError;
      if (typeof after === "string") {
        await beforeSend();
        throw errorFromSpec(after);
      }
      await beforeSend();
      return { sent: true };
    },
    async clearAfterDeferral({
      paneId,
      deferredForMs,
      maxDeferralMs,
      discard,
      log,
    }) {
      calls.push(
        `clearAfterDeferral ${paneId} deferredForMs=${deferredForMs} maxDeferralMs=${maxDeferralMs}`,
      );
      const script = (state.clear[paneId] ?? { cleared: false }) as {
        cleared?: boolean;
        text?: string;
        keys?: [string, string][];
      };
      const failure = failureOf(script);
      if (failure !== undefined) throw failure;
      for (const [key, reason] of script.keys ?? [])
        log({ pane: paneId, key, reason } as never);
      if (script.cleared === true) await discard(script.text ?? "");
      return { cleared: script.cleared === true, text: script.text ?? "" };
    },
  };
}

function stubProbe(state: Sections, calls: string[]): ProcessActivityProbe {
  return {
    async sample(paneId) {
      calls.push(`sample ${paneId}`);
      const script = (state.probe[paneId] ?? { processes: [] }) as {
        processes?: {
          pid: number;
          ppid: number;
          comm: string;
          cpuMs: number;
          startKey?: string;
        }[];
      };
      const failure = failureOf(script);
      if (failure !== undefined) throw failure;
      return {
        processes: (script.processes ?? []).map((p) => ({
          ...p,
          startKey: p.startKey ?? p.comm,
        })),
      };
    },
  };
}

/** The git the integration recovery asks, answering from the tick's `git` script. */
function stubGit(state: Sections, calls: string[]): IntegrationGit {
  const answer = (table: string, key: string, fallback: unknown): unknown => {
    const script = (state.git[table] ?? {}) as Record<string, unknown>;
    const value = key in script ? script[key] : fallback;
    const failure = failureOf(value);
    if (failure !== undefined) throw failure;
    return value;
  };
  const unused = (): never => {
    throw new Error("not used by the recovery");
  };
  return {
    headCommit: unused,
    commitExists: unused,
    merge: unused,
    isInHead: unused,
    async branchTip(branch) {
      calls.push(`branchTip ${branch}`);
      return answer("tips", branch, null) as string | null;
    },
    async deleteBranch(branch, sha) {
      calls.push(`deleteBranch ${branch} ${sha}`);
      return answer("delete", branch, true) as boolean;
    },
    async coveredReports(head, reports, options) {
      calls.push(
        `coveredReports ${head} ${JSON.stringify(reports.map((r) => r.reportId))} ${JSON.stringify(options?.memberCommits ?? null)}`,
      );
      const failure = failureOf(state.git.covered);
      if (failure !== undefined) throw failure;
      return (state.git.covered ?? []) as never;
    },
  };
}

// ------------------------------------------------------------------------------------------------ call counting

/** A proxy over the core that counts every use of a method or of the two counters, by name (what `newContext` reads). */
function countingCore(
  core: ControllerCore,
  counts: Map<string, number>,
): ControllerCore {
  return new Proxy(core, {
    get(target, property) {
      const value = Reflect.get(target, property, target) as unknown;
      if (
        typeof property === "string" &&
        (typeof value === "function" ||
          property === "stateVersion" ||
          property === "inputRevision")
      )
        counts.set(property, (counts.get(property) ?? 0) + 1);
      return typeof value === "function"
        ? (value as (...a: unknown[]) => unknown).bind(target)
        : value;
    },
  });
}

function takeCounts(counts: Map<string, number>): Json {
  const out: { [key: string]: Json } = {};
  for (const key of [...counts.keys()].sort()) out[key] = counts.get(key)!;
  counts.clear();
  return out;
}

// ------------------------------------------------------------------------------------------------ one sequence

const NOTIFICATION_FILE = "notifications.jsonl";

function flush(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

async function runSequence(
  source: SequenceSource,
  group: string,
): Promise<Json> {
  const seed = source.seed ?? `${group}/${source.name}`;
  const directory = tempDir("capstan-loops-");
  const hooks = installHooks(seed);
  const state = newSections();
  const config = source.config ?? {};
  try {
    // The ledger's migration timestamps come from the system clock in both implementations; migrating first keeps them
    // out of the seeded clock.
    (await openDatabase(path.join(directory, "controller.sqlite"))).close();
    const ownerCredential =
      "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";
    const core = await hooks.run(() =>
      ControllerCore.open({
        stateDirectory: directory,
        project: {
          projectId: "proj1",
          name: "Loops Project",
          ownerCredential,
          initialInputs: PLACEHOLDER_INPUTS,
        },
        workspaceRoot: directory,
      }),
    );
    const runner: StepRunner = {
      core,
      bindings: new Map<string, unknown>([
        ["owner", { credential: ownerCredential }],
        [
          "internal",
          {
            credential: new SeededStream(seed).bytes(32).toString("base64url"),
          },
        ],
      ]),
      hooks,
      next: 0,
    };
    const databaseFile = resolveDatabasePath(directory);
    const setup: Json[] = [];
    for (const step of [
      ...(source.preset === "team" ? TEAM : []),
      ...(source.setup ?? []),
    ])
      setup.push(runStep(runner, step));
    let previous = dumpTables(databaseFile) as unknown as Tables;

    const counts = new Map<string, number>();
    const counted = countingCore(core, counts);
    const calls: string[] = [];
    const events: Json[] = [];
    const log = (event: string, details: Record<string, unknown>): void => {
      events.push(
        stable({ event, details: JSON.parse(JSON.stringify(details)) }),
      );
    };
    let clock = SEEDED_EPOCH_MS;
    let seenNotifications = 0;
    const notifications = path.join(directory, NOTIFICATION_FILE);
    const timers = { ...DEFAULT_TIMERS, ...(config.timers ?? {}) };
    const tickMs = config.tickMs ?? 2000;
    const pmStaleSeconds = config.pmStaleSeconds ?? 20 * 60;
    const credential = ownerCredential;

    const adapter = stubAdapter(state, calls);
    const driver =
      source.loop === "driver"
        ? new DeliveryDriver({
            core: counted,
            adapter,
            timers,
            notifier: createNotifier({
              adapter: {
                async notify(title, body) {
                  calls.push(
                    `notify ${JSON.stringify(title)} ${JSON.stringify(body)}`,
                  );
                  const failure = failureOf(state.notify);
                  if (failure !== undefined) throw failure;
                },
              },
              channels: config.channels ?? { herdr: true, fallback: true },
              recordPath: notifications,
              now: () => new Date(clock),
              log: (event, details) => log(`notifier:${event}`, details),
            }),
            credential,
            now: () => clock,
            log,
            tickMs,
            processProbe: stubProbe(state, calls),
            pmStaleSeconds,
          })
        : undefined;

    // Supervision and the relay are timer loops: their interval callback is captured and called by hand.
    let timerCallback: (() => void) | undefined;
    const realSetInterval = globalThis.setInterval;
    const captureTimer = ((callback: () => void) => {
      timerCallback = callback;
      return {
        unref() {},
        ref() {},
        hasRef: () => false,
        [Symbol.dispose]() {},
      };
    }) as unknown as typeof setInterval;
    const launcherCalls = calls;
    let handle: { stop(): unknown } | undefined;
    globalThis.setInterval = captureTimer;
    try {
      if (source.loop === "supervision")
        handle = hooks.run(() =>
          startSupervision({
            core: counted,
            launcher: {
              async spawn(role: string) {
                launcherCalls.push(`spawn ${role}`);
                const failure = failureOf(state.spawn);
                if (failure !== undefined) throw failure;
                return { state: "started" };
              },
              async release(agentId: string) {
                launcherCalls.push(`release ${agentId}`);
                const failure = failureOf(state.release);
                if (failure !== undefined) throw failure;
                return undefined;
              },
            },
            credential,
            supervision: config.supervision ?? {
              enabled: true,
              checkSeconds: 60,
            },
            supervisorRole:
              config.supervisorRole === null
                ? undefined
                : (config.supervisorRole ?? "supervisor"),
            intervalMs: 15_000,
            now: () => clock,
            log,
          } as never),
        );
      else if (source.loop === "relay")
        handle = hooks.run(() =>
          startReportRelay({
            core: counted,
            credential,
            intervalMs: tickMs,
            log,
            ...(config.findingCheckSeconds == null
              ? {}
              : { findingCheckSeconds: config.findingCheckSeconds }),
          }),
        );
    } finally {
      globalThis.setInterval = realSetInterval;
    }

    const ticks: Json[] = [];
    for (const tick of source.ticks) {
      for (let round = 0; round < (tick.repeat ?? 1); round++) {
        clock =
          tick.at !== undefined && round === 0
            ? SEEDED_EPOCH_MS + tick.at
            : clock + tickMs;
        const record: { [key: string]: Json } = { at: clock - SEEDED_EPOCH_MS };
        if (round === 0) {
          const steps = (tick.steps ?? []).map((step) => runStep(runner, step));
          if (steps.length > 0) record.steps = steps;
          // Names bound by steps ("$r2.record.reportId") are resolved in what the stubs answer too.
          const adapter = resolveReferences(tick.adapter, runner) as
            AdapterPatch | undefined;
          mergeAdapter(state, adapter);
          if (adapter !== undefined) record.adapter = authored(adapter);
        }
        takeCounts(counts);
        calls.length = 0;
        events.length = 0;
        if (driver !== undefined) await hooks.run(() => driver.tick());
        else if (source.loop === "recover")
          await hooks.run(() =>
            recoverIntegrations({
              core: counted,
              git: stubGit(state, calls),
              context: (token) => newContext(counted, token),
              credential,
              log,
            }),
          );
        else {
          hooks.run(() => timerCallback?.());
          await flush();
          await flush();
        }
        record.calls = [...calls];
        record.log = [...events];
        record.coreCalls = takeCounts(counts);
        if (driver !== undefined) {
          const snapshot = driver.snapshot();
          record.snapshot = stable({
            stalledAgentIds: snapshot.stalledAgentIds,
            stuck: snapshot.stuck,
            lostAgentIds: snapshot.lostAgentIds ?? [],
            pmStale: snapshot.pmStale,
          });
          const all = readNotifications(notifications) as Json[];
          record.notifications = all.slice(seenNotifications);
          seenNotifications = all.length;
        }
        if (source.pmMail !== undefined)
          record.pmMail = stable(
            pmMailSummary(
              core.openMessagesFor(source.pmMail),
              clock,
              pmStaleSeconds,
            ),
          );
        if (tick.dump !== false) {
          const tables = dumpTables(databaseFile) as unknown as Tables;
          const diff = diffTables(previous, tables);
          previous = tables;
          if (Object.keys(diff as object).length > 0) record.tablesDiff = diff;
        }
        ticks.push(record);
      }
    }
    await handle?.stop();
    core.close();
    const out: { [key: string]: Json } = {
      name: source.name,
      seed,
      loop: source.loop,
      config: authored({
        timers,
        pmStaleSeconds,
        tickMs,
        channels: config.channels ?? { herdr: true, fallback: true },
        supervision: config.supervision ?? { enabled: true, checkSeconds: 60 },
        supervisorRole:
          config.supervisorRole === null
            ? null
            : (config.supervisorRole ?? "supervisor"),
        findingCheckSeconds: config.findingCheckSeconds ?? null,
        pmMail: source.pmMail ?? null,
      }),
      setup,
      ticks,
    };
    return out;
  } finally {
    hooks.restore();
    removeTempDir(directory);
  }
}

/** The lines the notifier wrote, as objects (the times are the scripted ones). */
function readNotifications(file: string): Json {
  let text: string;
  try {
    text = readFileSync(file, "utf8");
  } catch {
    return [];
  }
  return text
    .split("\n")
    .filter((line) => line !== "")
    .map((line) => stable(JSON.parse(line) as unknown));
}

// ------------------------------------------------------------------------------------------------ the export

export function sequenceFiles(): string[] {
  return readdirSync(SEQUENCES_DIRECTORY)
    .filter((name) => name.endsWith(".json"))
    .sort();
}

/** One sequence per line, so a diff of a regenerated file names the sequence that changed. */
function textOf(group: string, sequences: Json[]): string {
  return `${[
    `{"format":1,"group":${JSON.stringify(group)},"sequences":[`,
    sequences.map((sequence) => JSON.stringify(sequence)).join(",\n"),
    "]}",
  ].join("\n")}\n`;
}

/** Every exported file by repository-relative path, as the text the exporter writes. */
export async function exportFiles(): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  for (const name of sequenceFiles()) {
    const group = name.slice(0, -".json".length);
    const source = JSON.parse(
      readFileSync(path.join(SEQUENCES_DIRECTORY, name), "utf8"),
    ) as { sequences: SequenceSource[] };
    const sequences: Json[] = [];
    for (const sequence of source.sequences)
      sequences.push(await runSequence(sequence, group));
    files.set(`${PARITY_DIRECTORY}/${group}.json`, textOf(group, sequences));
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

/**
 * Writes the fixtures the Rust controller kernel (rust/crates/kernel) is tested against:
 *
 *  - rust/crates/kernel/tests/parity/<group>.json for every test/kernel-sequences/<group>.json: the sequences, each step
 *    resolved to concrete arguments, with what the real ControllerCore did (result or error, the project's state version
 *    and input revision, and table dumps). The Rust replay (tests/replay.rs) runs the same steps on the Rust kernel
 *    and compares everything. Time and randomness come from test/kernel-parity-hooks.ts, seeded per sequence.
 *  - rust/crates/kernel/tests/common/core-methods.json: the public methods of ControllerCore.
 *  - test/fixtures/plan-bodies/*.json: src/plans.ts (parsePlanBody, packageOfBody, packageNaming, workPackageMessage) and
 *    the subject formatting of src/conventions.ts.
 *  - test/fixtures/kernel-text/*.json: the text helpers and notice builders of src/controller/helpers.ts.
 *
 *   npm run build && node dist/test/kernel-parity-export.js [--out <dir>]
 *
 * writes the repository's committed files, or the same tree under <dir>. kernel-parity.test.ts fails while the committed
 * files differ from a fresh export.
 *
 * A sequence file is `{ "sequences": [ { name, seed?, project?, requires?, open?, steps } ] }` (language and output in
 * test/kernel-sequences/README.md). Each sequence opens its own ControllerCore in a tempDir scratch directory, removed
 * afterwards. A step is `{ op, on?, context?, args?, as?, dump? }`: `op` is a ControllerCore method name (or open,
 * openReadOnly, close, dump), `context` a symbolic MutationContext made whole by the exporter (ids from the step's
 * position, counters from the handle), `as` names the result for later `$name.path` references. The record of a step is
 * its resolved `context` and `args`, then `result` or `error` (class name) and `message`, and the counters; the record of
 * a sequence ends with a dump of every table. Time and randomness: see test/kernel-parity-hooks.ts.
 */
import { createHash } from "node:crypto";
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { format } from "prettier";
import { ControllerCore } from "../src/controller/core.js";
import { openDatabase } from "../src/controller/database.js";
import { openSqlite } from "../src/controller/sqlite.js";
// eslint-disable-next-line no-restricted-imports -- the exporter records what the internal helpers do, for the Rust port of them
import {
  agentStuckNotice,
  cancelledReason,
  cutAtCharacters,
  deliveryProblemNotice,
  findingNoticeBody,
  findingTask,
  findingText,
  integrationConflictNotice,
  lostNotice,
  oneLineText,
  packageReviewedNotice,
  planApprovedNotice,
  planCancelledNotice,
  planNeedsAttentionNotice,
  planSignedOffNotice,
  reportNotice,
  reviewNotice,
  reviewTask,
  safeText,
} from "../src/controller/helpers.js";
import { formatSubject, COMMIT_TYPES } from "../src/conventions.js";
import { stripTerminalSequences } from "../src/observe.js";
import {
  packageNaming,
  packageOfBody,
  parsePlanBody,
  workPackageMessage,
  type PackageView,
} from "../src/plans.js";
import { normalizeText, oneLine } from "../src/text.js";
import { installHooks, SeededStream } from "./kernel-parity-hooks.js";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const SEQUENCES_DIRECTORY = path.join(root, "test", "kernel-sequences");
/** Where a file of the export lives in the repository, relative to the repository root. */
export const EXPORT_LAYOUT = {
  parity: "rust/crates/kernel/tests/parity",
  common: "rust/crates/kernel/tests/common",
  planBodies: "test/fixtures/plan-bodies",
  text: "test/fixtures/kernel-text",
} as const;

// ------------------------------------------------------------------------------------------------ JSON helpers

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

/** `value` with every object's keys in code-unit order, so the output text does not depend on construction order. */
export function stable(value: unknown): Json {
  if (value === undefined) return null;
  if (value === null || typeof value !== "object") return value as Json;
  if (Array.isArray(value)) return value.map(stable);
  const out: { [key: string]: Json } = {};
  for (const key of Object.keys(value).sort())
    out[key] = stable((value as Record<string, unknown>)[key]);
  return out;
}

function text(value: unknown): string {
  return JSON.stringify(stable(value), null, 2) + "\n";
}

// ------------------------------------------------------------------------------------------------ sequences

export interface Step {
  /** A `ControllerCore` method (`open` and `openReadOnly` included), or `dump`. */
  readonly op: string;
  readonly on?: string;
  readonly context?: Record<string, unknown>;
  readonly args?: unknown[];
  readonly as?: string;
  readonly dump?: boolean;
}

export interface SequenceSource {
  readonly name: string;
  readonly seed?: string;
  /** Fields of the default project this sequence overrides. */
  readonly project?: Record<string, unknown>;
  /** Groups other than the sequence's own whose operations it reaches. */
  readonly requires?: string[];
  /** Whether the sequence opens a controller before its first step (default true). */
  readonly open?: boolean;
  readonly steps: Step[];
}

const OWNER_CREDENTIAL =
  "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";
const DEFAULT_PROJECT = {
  projectId: "proj1",
  name: "Parity Project",
  ownerCredential: "$owner.credential",
  initialInputs: [
    { kind: "project_config", content: { name: "parity" } },
    { kind: "task_brief", content: { objective: "prove parity" } },
    { kind: "acceptance_criteria", content: { criteria: ["it matches"] } },
    { kind: "policy", content: { review: "required" } },
    { kind: "plan", content: { steps: ["one", "two"] } },
  ],
};

interface Handle {
  core: ControllerCore;
  open: boolean;
}

interface Context {
  readonly bindings: Map<string, unknown>;
  readonly handles: Map<string, Handle>;
  readonly directory: string;
}

function counters(
  context: Context,
  on: string,
): { version: number; revision: number } {
  const handle = context.handles.get(on);
  if (handle === undefined || !handle.open) return { version: 1, revision: 1 };
  try {
    return {
      version: handle.core.stateVersion,
      revision: handle.core.inputRevision,
    };
  } catch {
    return { version: 1, revision: 1 };
  }
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

/** Replaces `"$name.path"` strings by the named result (or `$version` / `$revision` by the handle's counters). */
function resolve(value: unknown, context: Context, on: string): unknown {
  if (typeof value === "string" && /^\$[A-Za-z]/.test(value)) {
    const [name = "", ...parts] = value.slice(1).split(".");
    if (name === "version" && parts.length === 0)
      return counters(context, on).version;
    if (name === "revision" && parts.length === 0)
      return counters(context, on).revision;
    if (!context.bindings.has(name))
      throw new Error(`unknown reference ${value}`);
    return lookup(context.bindings.get(name), parts, value);
  }
  if (Array.isArray(value)) return value.map((v) => resolve(v, context, on));
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, v] of Object.entries(value))
      out[key] = resolve(v, context, on);
    return out;
  }
  return value;
}

/** A symbolic mutation context made whole: ids from the step's position, counters from the handle. */
function mutationContext(
  step: Step,
  index: number,
  context: Context,
  on: string,
): Record<string, unknown> {
  const spec = resolve(step.context, context, on) as Record<string, unknown>;
  const current = counters(context, on);
  return {
    credential: spec.credential,
    requestId: spec.requestId ?? `req-${index}`,
    idempotencyKey: spec.idempotencyKey ?? `idem-${index}`,
    expectedVersion: spec.expectedVersion ?? current.version,
    inputRevision: spec.inputRevision ?? current.revision,
  };
}

function errorOf(error: unknown): { name: string; message: string } {
  const e = error as { name?: unknown; message?: unknown };
  return {
    name: typeof e?.name === "string" ? e.name : "Error",
    message: typeof e?.message === "string" ? e.message : String(error),
  };
}

/** Every table of the ledger that has rows (empty tables are left out), each row's values in column order, rows ordered by all columns. */
export function dumpTables(file: string): Json {
  const database = openSqlite(file, { readOnly: true, timeout: 5_000 });
  try {
    const tables = database
      .prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
      )
      .all() as Array<{ name: string }>;
    const out: { [table: string]: Json } = {};
    for (const { name } of tables) {
      const columns = (
        database.prepare(`PRAGMA table_info(${name})`).all() as Array<{
          name: string;
        }>
      )
        .map((column) => column.name)
        .filter(
          (column) =>
            !(name === "schema_migrations" && column === "applied_at"),
        );
      const order = columns.map((_, i) => i + 1).join(", ");
      const rows = database
        .prepare(`SELECT ${columns.join(", ")} FROM ${name} ORDER BY ${order}`)
        .all() as Array<Record<string, unknown>>;
      if (rows.length === 0) continue;
      out[name] = {
        columns,
        rows: rows.map((row) => columns.map((c) => stable(row[c]))),
      };
    }
    return out;
  } finally {
    database.close();
  }
}

async function runSequence(source: SequenceSource): Promise<Json> {
  const seed = source.seed ?? source.name;
  const directory = tempDir("capstan-kernel-parity-");
  const hooks = installHooks(seed);
  const project = { ...DEFAULT_PROJECT, ...(source.project ?? {}) };
  const context: Context = {
    bindings: new Map<string, unknown>([
      ["owner", { credential: OWNER_CREDENTIAL }],
      [
        // The internal controller's credential is the first draw of the seeded stream, which makes it knowable.
        "internal",
        {
          credential: new SeededStream(seed).bytes(32).toString("base64url"),
        },
      ],
    ]),
    handles: new Map(),
    directory,
  };
  const steps: Step[] = [
    ...(source.open === false ? [] : [{ op: "open", args: [{}] } as Step]),
    ...source.steps,
  ];
  const records: Json[] = [];
  try {
    for (const [index, step] of steps.entries()) {
      const on = step.on ?? "main";
      const record: { [key: string]: Json } = { op: step.op, on };
      let result: unknown = null;
      let failure: { name: string; message: string } | undefined;
      try {
        if (step.op === "open" || step.op === "openReadOnly") {
          const spec = resolve(step.args?.[0] ?? {}, context, on) as {
            project?: Record<string, unknown>;
            stateDir?: string;
            options?: Record<string, unknown>;
          };
          const resolved = resolve(
            { ...project, ...(spec.project ?? {}) },
            context,
            on,
          ) as Record<string, unknown>;
          const options = spec.options ?? {};
          const stateDir = spec.stateDir ?? "dir";
          record.args = stable([{ project: resolved, stateDir, options }]);
          const stateDirectory = stateDir === "dir" ? directory : stateDir;
          if (stateDir === "dir" && step.op === "open") {
            // The ledger's migration timestamps come from the system clock in both implementations; migrating first
            // keeps them out of the seeded clock.
            (
              await openDatabase(path.join(directory, "controller.sqlite"))
            ).close();
          }
          const opts = {
            stateDirectory,
            project: resolved as never,
            ...(typeof options.workspaceRoot === "string"
              ? { workspaceRoot: options.workspaceRoot }
              : {}),
            ...(typeof options.runtimeWorkspacePath === "string"
              ? { runtimeWorkspacePath: options.runtimeWorkspacePath }
              : {}),
          };
          const core = await hooks.run(() =>
            step.op === "openReadOnly"
              ? ControllerCore.openReadOnly(opts)
              : ControllerCore.open(opts),
          );
          context.handles.set(on, { core, open: true });
        } else if (step.op === "close") {
          record.args = [];
          const handle = context.handles.get(on);
          if (handle === undefined) throw new Error(`no handle ${on}`);
          handle.core.close();
          handle.open = false;
        } else if (step.op === "dump") {
          record.args = [];
          record.dump = true;
        } else {
          const args: unknown[] = [];
          if (step.context !== undefined) {
            const made = mutationContext(step, index, context, on);
            record.context = stable(made);
            args.push(made);
          }
          args.push(...(resolve(step.args ?? [], context, on) as unknown[]));
          record.args = stable(
            step.context === undefined ? args : args.slice(1),
          );
          const handle = context.handles.get(on);
          if (handle === undefined) throw new Error(`no handle ${on}`);
          const member = (handle.core as unknown as Record<string, unknown>)[
            step.op
          ];
          const raw: unknown = hooks.run(() =>
            typeof member === "function"
              ? (member as (...a: unknown[]) => unknown).apply(
                  handle.core,
                  args,
                )
              : member,
          );
          result = stable(
            JSON.parse(JSON.stringify(raw === undefined ? null : raw)),
          );
          if (step.as !== undefined) context.bindings.set(step.as, result);
        }
      } catch (error) {
        failure = errorOf(error);
      }
      if (failure === undefined) record.result = stable(result);
      else {
        record.error = failure.name;
        record.message = failure.message;
      }
      if (step.as !== undefined) record.as = step.as;
      const handle = context.handles.get(on);
      if (handle?.open === true) {
        try {
          record.stateVersion = handle.core.stateVersion;
          record.inputRevision = handle.core.inputRevision;
        } catch {
          // a handle that failed has no counters
        }
      }
      if (step.dump === true || step.op === "dump")
        record.tables = dumpTables(path.join(directory, "controller.sqlite"));
      records.push(record);
    }
    return stable({
      name: source.name,
      seed,
      requires: source.requires ?? [],
      project: project as never,
      steps: records,
      tables: dumpTables(path.join(directory, "controller.sqlite")),
    });
  } finally {
    for (const handle of context.handles.values()) {
      try {
        handle.core.close();
      } catch {
        // already closed
      }
    }
    hooks.restore();
    removeTempDir(directory);
  }
}

export function sequenceFiles(): string[] {
  return readdirSync(SEQUENCES_DIRECTORY)
    .filter((name) => name.endsWith(".json"))
    .sort();
}

/** A parity group file: the header, then one sequence per line (compact, to keep the committed fixtures small). */
function groupText(group: string, sequences: Json[]): string {
  const lines = sequences.map((sequence) => JSON.stringify(stable(sequence)));
  return `{"format":1,"group":${JSON.stringify(group)},"sequences":[\n${lines.join(",\n")}\n]}\n`;
}

async function sequenceGroups(): Promise<Map<string, string>> {
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
    out.set(
      `${EXPORT_LAYOUT.parity}/${group}.json`,
      groupText(group, sequences),
    );
  }
  return out;
}

function coreMethods(): string[] {
  const names = new Set<string>();
  for (const name of Object.getOwnPropertyNames(ControllerCore.prototype))
    if (name !== "constructor") names.add(name);
  for (const name of Object.getOwnPropertyNames(ControllerCore))
    if (!["length", "name", "prototype"].includes(name)) names.add(name);
  return [...names].sort();
}

// ------------------------------------------------------------------------------------------------ plan bodies

function pkg(id: string, extra: Record<string, unknown> = {}): unknown {
  return {
    id,
    title: `Package ${id}`,
    owns: [`src/${id}`],
    acceptance: [`${id} works`],
    estimate_hours: 4,
    ...extra,
  };
}

function body(
  packages: unknown[],
  extra: Record<string, unknown> = {},
): string {
  return JSON.stringify({ summary: "A plan", packages, ...extra });
}

const MALFORMED_JSON = [
  "",
  "   ",
  "{",
  '{"a"',
  '{"a":',
  '{"a":1',
  '{"a":1,',
  '{"a":1,}',
  "[1,]",
  "[1 2]",
  '{"a" 1}',
  "{a:1}",
  "nul",
  "not json",
  "tru",
  '{"a":01}',
  "1 2",
  '{"a":"x',
  '"a\nb"',
  '"\\x"',
  "-",
  "1.",
  "1e",
  "[",
  "[1",
  '{"a":1}x',
  '{"summary":"aaaaaaaaaaaaaaaaaaaaaaaaaaa",bad}',
  "x" + "a".repeat(40),
  "a".repeat(40) + "x",
  '{\n  "a": ,\n}',
  '"\\u12"',
  '{"a":tru}',
  "[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,x]",
  '{"😀":}',
  "undefined",
  "NaN",
];

interface PlanCase {
  readonly name: string;
  readonly input: string;
  readonly maxPackages: number;
}

function planCases(): PlanCase[] {
  const cases: PlanCase[] = [];
  const add = (name: string, input: string, maxPackages = 20): void => {
    cases.push({ name, input, maxPackages });
  };
  add("minimal", body([pkg("a")]));
  add(
    "full",
    JSON.stringify({
      summary: "  Ship\r\nthe thing\u0007 ",
      risks: ["slow", " fast\t"],
      integration_order: ["b", "a"],
      packages: [
        pkg("a", {
          title: "Alpha",
          role: "architect",
          owns: ["./src//a/", "docs/a.md"],
          interfaces: ["fn a()"],
          depends_on: [],
          type: "feat",
          scope: "kernel.core",
          breaking: true,
          risks: ["r1"],
        }),
        pkg("b", { depends_on: ["a"], estimate_hours: 80, type: "fix" }),
      ],
    }),
  );
  add(
    "dependency-chain",
    body([
      pkg("a"),
      pkg("b", { depends_on: ["a"], owns: ["src/a/inner"] }),
      pkg("c", { depends_on: ["b"], owns: ["src/a"] }),
    ]),
  );
  add(
    "integration-order-default",
    body([pkg("b", { depends_on: ["a"] }), pkg("a")]),
  );
  add("fractional-hours", body([pkg("a", { estimate_hours: 0.5 })]));
  add("unicode-text", body([pkg("a", { title: "café 😀 ‮ x y" })]));
  add(
    "lone-surrogate-escape",
    '{"summary":"x\\ud83dy","packages":[{"id":"a","title":"t","owns":["p"],"acceptance":["c"],"estimate_hours":1}]}',
  );
  add(
    "duplicate-keys",
    '{"summary":"first","summary":"second","packages":[{"id":"a","title":"t","owns":["p"],"acceptance":["c"],"estimate_hours":1}]}',
  );
  add("limit-exact", body([pkg("a"), pkg("b")]), 2);
  add("too-many-packages", body([pkg("a"), pkg("b"), pkg("c")]), 2);
  add("too-large", body([pkg("a", { title: "x".repeat(33 * 1024) })]));
  for (const [i, bad] of MALFORMED_JSON.entries())
    add(`invalid-json-${String(i).padStart(2, "0")}`, bad);
  const shape = (name: string, value: unknown): void =>
    add(
      `shape-${name}`,
      typeof value === "string" ? value : JSON.stringify(value),
    );
  shape("plan-array", []);
  shape("plan-null", "null");
  shape("plan-string", '"x"');
  shape("plan-unknown-key", { summary: "s", packages: [pkg("a")], extra: 1 });
  shape("plan-two-unknown-keys", {
    zeta: 1,
    alpha: 2,
    summary: "s",
    packages: [pkg("a")],
  });
  shape("summary-missing", { packages: [pkg("a")] });
  shape("summary-number", { summary: 1, packages: [pkg("a")] });
  shape("summary-blank", { summary: " \n ", packages: [pkg("a")] });
  shape("packages-missing", { summary: "s" });
  shape("packages-empty", { summary: "s", packages: [] });
  shape("packages-object", { summary: "s", packages: {} });
  shape("package-string", { summary: "s", packages: ["x"] });
  shape("package-array", { summary: "s", packages: [[]] });
  shape("package-unknown-key", {
    summary: "s",
    packages: [pkg("a", { owner: "x" })],
  });
  shape("id-missing", {
    summary: "s",
    packages: [
      { title: "t", owns: ["p"], acceptance: ["a"], estimate_hours: 1 },
    ],
  });
  shape("id-pattern-upper", { summary: "s", packages: [pkg("Alpha")] });
  shape("id-pattern-digit", { summary: "s", packages: [pkg("1a")] });
  shape("id-pattern-long", { summary: "s", packages: [pkg("a".repeat(33))] });
  shape("id-number", { summary: "s", packages: [pkg("a", { id: 3 })] });
  for (const hours of [0, -1, 81, "4", null, true])
    shape(`hours-${JSON.stringify(hours)}`, {
      summary: "s",
      packages: [pkg("a", { estimate_hours: hours })],
    });
  shape("hours-missing", {
    summary: "s",
    packages: [{ id: "a", title: "t", owns: ["p"], acceptance: ["a"] }],
  });
  shape("type-unknown", {
    summary: "s",
    packages: [pkg("a", { type: "style" })],
  });
  shape("type-number", { summary: "s", packages: [pkg("a", { type: 1 })] });
  shape("scope-bad", {
    summary: "s",
    packages: [pkg("a", { scope: "Bad Scope" })],
  });
  shape("scope-long", {
    summary: "s",
    packages: [pkg("a", { scope: "a".repeat(31) })],
  });
  shape("breaking-string", {
    summary: "s",
    packages: [pkg("a", { breaking: "yes" })],
  });
  shape("title-blank", { summary: "s", packages: [pkg("a", { title: " " })] });
  shape("title-missing", {
    summary: "s",
    packages: [{ id: "a", owns: ["p"], acceptance: ["a"], estimate_hours: 1 }],
  });
  shape("role-number", { summary: "s", packages: [pkg("a", { role: 2 })] });
  shape("owns-missing", {
    summary: "s",
    packages: [{ id: "a", title: "t", acceptance: ["a"], estimate_hours: 1 }],
  });
  shape("owns-empty", { summary: "s", packages: [pkg("a", { owns: [] })] });
  shape("owns-object", { summary: "s", packages: [pkg("a", { owns: {} })] });
  shape("owns-entry-number", {
    summary: "s",
    packages: [pkg("a", { owns: ["p", 1] })],
  });
  shape("owns-absolute", {
    summary: "s",
    packages: [pkg("a", { owns: ["/etc/passwd"] })],
  });
  shape("owns-parent", {
    summary: "s",
    packages: [pkg("a", { owns: ["src/../x"] })],
  });
  shape("owns-dot", { summary: "s", packages: [pkg("a", { owns: ["./."] })] });
  shape("interfaces-string", {
    summary: "s",
    packages: [pkg("a", { interfaces: "x" })],
  });
  shape("depends-string", {
    summary: "s",
    packages: [pkg("a", { depends_on: "x" })],
  });
  shape("acceptance-missing", {
    summary: "s",
    packages: [{ id: "a", title: "t", owns: ["p"], estimate_hours: 1 }],
  });
  shape("acceptance-empty", {
    summary: "s",
    packages: [pkg("a", { acceptance: [] })],
  });
  shape("risks-string", { summary: "s", packages: [pkg("a", { risks: "x" })] });
  shape("plan-risks-object", { summary: "s", packages: [pkg("a")], risks: {} });
  shape("order-string", {
    summary: "s",
    packages: [pkg("a")],
    integration_order: "a",
  });
  shape("order-empty", {
    summary: "s",
    packages: [pkg("a")],
    integration_order: [],
  });
  add("duplicate-id", body([pkg("a"), pkg("a", { owns: ["other"] })]));
  add("unknown-dependency", body([pkg("a", { depends_on: ["zz"] })]));
  add(
    "cycle-two",
    body([
      pkg("a", { depends_on: ["b"] }),
      pkg("b", { depends_on: ["a"], owns: ["other"] }),
    ]),
  );
  add("cycle-self", body([pkg("a", { depends_on: ["a"] })]));
  add(
    "cycle-partial",
    body([
      pkg("a"),
      pkg("b", { depends_on: ["c"], owns: ["o2"] }),
      pkg("c", { depends_on: ["b"], owns: ["o3"] }),
    ]),
  );
  add(
    "overlap-equal",
    body([pkg("a", { owns: ["src/x"] }), pkg("b", { owns: ["src/x"] })]),
  );
  add(
    "overlap-nested",
    body([pkg("a", { owns: ["src/x"] }), pkg("b", { owns: ["src/x/y.rs"] })]),
  );
  add(
    "overlap-ordered-ok",
    body([
      pkg("a", { owns: ["src/x"] }),
      pkg("b", { owns: ["src/x/y.rs"], depends_on: ["a"] }),
    ]),
  );
  add(
    "overlap-transitive-ok",
    body([
      pkg("a", { owns: ["src/x"] }),
      pkg("b", { owns: ["m"], depends_on: ["a"] }),
      pkg("c", { owns: ["src/x/z"], depends_on: ["b"] }),
    ]),
  );
  add(
    "overlap-prefix-not-path",
    body([pkg("a", { owns: ["src/x"] }), pkg("b", { owns: ["src/xy"] })]),
  );
  add("order-unknown", body([pkg("a")], { integration_order: ["zz"] }));
  add(
    "order-repeat",
    body([pkg("a"), pkg("b")], { integration_order: ["a", "a"] }),
  );
  add(
    "order-missing",
    body([pkg("a"), pkg("b")], { integration_order: ["a"] }),
  );
  add(
    "order-before-dependency",
    body([pkg("a"), pkg("b", { depends_on: ["a"] })], {
      integration_order: ["b", "a"],
    }),
  );
  add(
    "order-valid",
    body([pkg("a"), pkg("b", { depends_on: ["a"] })], {
      integration_order: ["a", "b"],
    }),
  );
  return cases;
}

function planFixtures(): Map<string, string> {
  const out = new Map<string, string>();
  out.set(
    `${EXPORT_LAYOUT.planBodies}/parse.json`,
    text(
      planCases().map((c) => ({
        name: c.name,
        input: c.input,
        maxPackages: c.maxPackages,
        result: parsePlanBody(c.input, { maxPackages: c.maxPackages }),
      })),
    ),
  );
  const bodies = [
    body([
      pkg("a", { type: "feat", scope: "x", breaking: true }),
      pkg("b"),
      pkg("c", { type: "nope" }),
    ]),
    JSON.stringify({
      packages: [
        { id: "a" },
        null,
        3,
        {
          id: "b",
          title: 5,
          owns: ["x", 2],
          dependsOn: ["a"],
          estimateHours: "1",
          type: "fix",
          scope: 7,
          breaking: "true",
        },
        {
          id: "c",
          estimateHours: 2.5,
          title: "T",
          interfaces: ["i"],
          acceptance: ["ok"],
          risks: ["r"],
        },
      ],
    }),
    JSON.stringify({ packages: "no" }),
    "not json",
    "null",
    "[]",
    "{}",
  ];
  const ids = ["a", "b", "c", "zz"];
  const naming: unknown[] = [];
  const view: unknown[] = [];
  for (const [i, bodyJson] of bodies.entries()) {
    for (const id of ids) {
      naming.push({
        name: `body${i}-${id}`,
        body: bodyJson,
        packageId: id,
        result: packageNaming(bodyJson, id),
      });
      view.push({
        name: `body${i}-${id}`,
        body: bodyJson,
        packageId: id,
        result: packageOfBody(bodyJson, id) ?? null,
      });
    }
  }
  out.set(`${EXPORT_LAYOUT.planBodies}/naming.json`, text(naming));
  out.set(`${EXPORT_LAYOUT.planBodies}/package-view.json`, text(view));
  const views: PackageView[] = [
    {
      title: "Plain",
      owns: ["a"],
      interfaces: [],
      dependsOn: [],
      estimateHours: null,
      acceptance: [],
      risks: [],
    },
    {
      title: 'Quo"te\nnewline',
      owns: ["src/a", 'b"c'],
      interfaces: ["i1", "i2"],
      dependsOn: ["x", "y"],
      estimateHours: 2.5,
      acceptance: ["one", "two\nlines"],
      risks: ["r"],
    },
    {
      title: "\u{1F600}",
      owns: ["a"],
      interfaces: ["x"],
      dependsOn: ["z"],
      estimateHours: 80,
      acceptance: ["c"],
      risks: [],
    },
  ];
  const messages: unknown[] = [];
  for (const [i, v] of views.entries())
    for (const unmet of [[], ["x"], ["x", "y"]])
      messages.push({
        name: `view${i}-unmet${unmet.length}`,
        planId: "plan-1",
        packageId: "pkg-a",
        architect: "arch-1",
        view: v,
        unmet,
        result: workPackageMessage("plan-1", "pkg-a", "arch-1", v, unmet),
      });
  out.set(
    `${EXPORT_LAYOUT.planBodies}/work-package-message.json`,
    text(messages),
  );
  const descriptions = [
    "add the thing",
    "x".repeat(100),
    "word ".repeat(30),
    "Rewrite the controller kernel in Rust with parity checks and of the",
    "   spaced \t out \n text  ",
    "",
    "   ",
    "trailing punctuation, and, the.",
    "Implement support for the and of in on",
    "café 😀".repeat(20),
    "a-b-c; d: e! f? g& h+ i/ j( k[ l{",
  ];
  const subjects: unknown[] = [];
  for (const type of [...COMMIT_TYPES, "other", ""])
    for (const scope of [
      undefined,
      "kernel",
      "Bad Scope!",
      "a/b.c_d-e",
      "x".repeat(70),
    ])
      for (const breaking of [false, true])
        for (const [i, description] of descriptions.entries()) {
          if (i > 3 && (type !== "feat" || scope !== "kernel")) continue;
          const input = {
            type,
            ...(scope === undefined ? {} : { scope }),
            breaking,
            description,
          };
          subjects.push({ input, max: 72, result: formatSubject(input) });
        }
  for (const max of [10, 20, 30, 50])
    for (const [i, description] of descriptions.entries())
      subjects.push({
        input: {
          type: "feat",
          scope: "kernel",
          description: `${description}${i}`,
        },
        max,
        result: formatSubject(
          { type: "feat", scope: "kernel", description: `${description}${i}` },
          max,
        ),
      });
  out.set(`${EXPORT_LAYOUT.planBodies}/subjects.json`, text(subjects));
  return out;
}

// ------------------------------------------------------------------------------------------------ text helpers

const TEXT_SAMPLES = [
  "",
  " ",
  "plain text",
  "  padded  ",
  "line one\r\nline two\rline three\nline four",
  "nel\u0085next ls ps",
  "tab\there\u000bvt\u000cff",
  "control\u0000nul\u0007bell\u001bescape\u007fdel\u0080c1\u009f",
  "format​zero‍zwj‎‮rtl⁠wj﻿bom­soft",
  "nonchar﷐￿￾ end",
  "non-BMP 😀 👨‍👩‍👧 \u{1F1EF}\u{1F1F5} \u{20BB7}",
  "é combining क्ष 각",
  " nbsp em　ideographic ",
  "⠀braille ᅟᅠㅤﾠ fillers",
  "ansi \u001b[31mred\u001b[0m \u001b[1;4munder\u001b[0m \u001b[2J\u001b[H",
  "osc \u001b]0;title\u0007after \u001b]8;;http://x\u001b\\link\u001b]8;;\u001b\\",
  "dcs \u001bPq#0;2;0;0;0\u001b\\ apc \u001b_data\u001b\\ pm \u001b^x\u001b\\ sos \u001bXy\u001b\\",
  "c1 csi \u009b31m red \u009d0;t\u0007 done \u0090dcs\u009c end",
  "unterminated \u001b]0;title no end",
  "unterminated csi \u001b[31",
  "escape alone \u001b",
  "two char \u001bM \u001b7 \u001b(B \u001b=",
  "intro \u0090 \u0098 \u009d \u009e \u009f lone",
  "long ".repeat(400),
  "wide \u{1F600}".repeat(150),
  "mix\r\n".repeat(60) + "\u001b[0m",
  "​​​",
  "� replacement �",
  "RTL אבג العربية",
];

/** JSON texts whose parsed value Node prints back with `JSON.stringify`; Rust reads the same text and must print the same. */
const JSON_TEXT_SAMPLES: string[] = [
  "null",
  "true",
  "[]",
  "{}",
  '{"b":1,"a":2,"c":{"z":1,"y":{"k":2,"j":3},"x":[{"n":1,"m":2}]}}',
  '{"b":1,"2":"two","a":2,"10":"ten","1":"one","01":"zero-one","4294967295":"max","4294967294":"last"}',
  '{"x":{"3":1,"b":2,"1":3,"a":4}}',
  "[0,-0,1,-1,1.5,1e21,1e-7,123456789012345680000,1.2e-10,0.000001,100,5e-324,1.7976931348623157e308,0.1,-1e-7]",
  '{"unicode":"é€😀\\u0000\\u001f\\u007f\\u2028\\u2029","esc":"\\"\\\\\\b\\f\\n\\r\\t\\/","key é":1,"k\\n":2}',
  '[{"a":[],"b":{}},[[]],[{}]]',
  '  {  "pad" : [ 1 , 2 ] }  ',
];

function textFixtures(): Map<string, string> {
  const out = new Map<string, string>();
  out.set(
    `${EXPORT_LAYOUT.text}/text.json`,
    text({
      normalizeText: TEXT_SAMPLES.map((input) => ({
        input,
        result: normalizeText(input),
      })),
      oneLine: TEXT_SAMPLES.flatMap((input) =>
        [1, 2, 10, 50, 200, 1000].map((max) => ({
          input,
          max,
          empty: "(none)",
          result: oneLine(input, max, "(none)"),
        })),
      ),
      stripTerminalSequences: TEXT_SAMPLES.map((input) => ({
        input,
        result: stripTerminalSequences(input),
      })),
      oneLineText: TEXT_SAMPLES.flatMap((input) =>
        [1, 5, 120, 200].map((max) => ({
          input,
          max,
          result: oneLineText(input, max),
        })),
      ),
      cutAtCharacters: TEXT_SAMPLES.flatMap((input) =>
        [0, 1, 3, 7, 40, 500].map((limit) => ({
          input,
          limit,
          result: cutAtCharacters(input, limit),
        })),
      ),
    }),
  );
  out.set(
    `${EXPORT_LAYOUT.text}/json-stringify.json`,
    text({
      stringify: JSON_TEXT_SAMPLES.map((input) => ({
        input,
        result: JSON.stringify(JSON.parse(input)),
      })),
    }),
  );
  const outcome = (fn: () => unknown): Json => {
    try {
      return { ok: stable(fn() ?? null) };
    } catch (error) {
      return { error: errorOf(error) };
    }
  };
  const values: unknown[] = [...TEXT_SAMPLES, 5, null, undefined, {}];
  out.set(
    `${EXPORT_LAYOUT.text}/validation.json`,
    text({
      safeText: values.flatMap((input) =>
        [false, true].flatMap((multiline) =>
          [10, 500].map((max) => ({
            input: input ?? null,
            max,
            multiline,
            result: outcome(() => safeText(input, "the field", max, multiline)),
          })),
        ),
      ),
      findingText: values.flatMap((input) =>
        [20, 1500].map((max) => ({
          input: input ?? null,
          max,
          result: outcome(() => findingText(input, "evidence", max)),
        })),
      ),
    }),
  );
  const report = {
    report_id: "r1",
    sequence: 1,
    agent_id: "dev-1",
    generation: 2,
    actor_id: "act",
    commit_sha: "abc123",
    branch: "feat/x",
    summary: 'did "it"\nthen more',
    state: "accepted",
    reason: null,
    evidence_json: "{}",
    notified_message_id: null,
    created_at: "t",
  } as never;
  const review = (extra: Record<string, unknown>): never =>
    ({
      review_id: "rv1",
      sequence: 1,
      round: 2,
      subject_report_id: "r1",
      subject_integration_id: null,
      subject_plan_id: null,
      subject_plan_revision: null,
      commit_sha: "c1",
      base_sha: "b1",
      author_agent_id: "dev-1",
      author_actor_id: "a",
      requested_by_actor_id: "pm",
      reviewer_role: "reviewer",
      reviewer_agent_id: "ver-1",
      reviewer_actor_id: "v",
      state: "passed",
      verdict_text: 'looks "fine"',
      failure_reason: null,
      notified_message_id: null,
      created_at: "t",
      completed_at: "t2",
      ...extra,
    }) as never;
  const finding = (extra: Record<string, unknown>): never =>
    ({
      finding_id: "f1",
      sequence: 1,
      target_agent_id: "dev-1",
      raised_by_agent_id: "sup-1",
      raised_by_actor_id: "a",
      severity: "high",
      evidence_text: 'e "x"',
      requested_correction: "fix\nit",
      resolution_condition: "when green",
      state: "open",
      interventions: 1,
      state_reason: null,
      created_at: "t",
      closed_at: null,
      ...extra,
    }) as never;
  const agent = {
    agent_id: "dev-1",
    role_name: "developer",
    kind: "Developer",
    seat_id: "s",
    actor_id: "a",
    generation: 1,
    state: "active",
    last_activity_at: "t",
  } as never;
  const planBody = JSON.stringify({
    packages: [
      { id: "a", title: "Alpha", dependsOn: [] },
      { id: "b", title: 'Beta "q"', dependsOn: ["a"] },
    ],
    integrationOrder: ["a", "b"],
  });
  const bigBody = JSON.stringify({
    packages: Array.from({ length: 20 }, (_, i) => ({
      id: `p${i}`,
      title: "t".repeat(900),
      dependsOn: i === 0 ? [] : [`p${i - 1}`],
    })),
    integrationOrder: ["p0"],
  });
  out.set(
    `${EXPORT_LAYOUT.text}/notices.json`,
    text({
      reportNotice: [
        {
          row: report,
          role: "developer",
          result: reportNotice(report, "developer"),
        },
      ],
      reviewTask: [
        {
          row: review({}),
          authors: [{ report }],
          result: reviewTask(review({}), [{ report }]),
        },
        {
          row: review({
            subject_report_id: null,
            subject_integration_id: "i1",
          }),
          authors: [
            { report },
            {
              report: {
                ...(report as object),
                report_id: "r2",
                agent_id: "dev-2",
                summary: "two",
              },
            },
          ],
          result: reviewTask(
            review({ subject_report_id: null, subject_integration_id: "i1" }),
            [
              { report },
              {
                report: {
                  ...(report as object),
                  report_id: "r2",
                  agent_id: "dev-2",
                  summary: "two",
                } as never,
              },
            ],
          ),
        },
        {
          row: review({
            subject_report_id: null,
            subject_plan_id: "p1",
            subject_plan_revision: 3,
          }),
          authors: [],
          result: reviewTask(
            review({
              subject_report_id: null,
              subject_plan_id: "p1",
              subject_plan_revision: 3,
            }),
            [],
          ),
        },
      ],
      reviewNotice: [
        {
          row: review({}),
          authors: ["dev-1"],
          result: reviewNotice(review({}), ["dev-1"]),
        },
        {
          row: review({
            state: "findings",
            subject_report_id: null,
            subject_integration_id: "i1",
          }),
          authors: ["dev-1", "dev-2", "dev-1"],
          result: reviewNotice(
            review({
              state: "findings",
              subject_report_id: null,
              subject_integration_id: "i1",
            }),
            ["dev-1", "dev-2", "dev-1"],
          ),
        },
        {
          row: review({
            subject_report_id: null,
            subject_plan_id: "p1",
            subject_plan_revision: 2,
          }),
          authors: ["arch-1"],
          result: reviewNotice(
            review({
              subject_report_id: null,
              subject_plan_id: "p1",
              subject_plan_revision: 2,
            }),
            ["arch-1"],
          ),
        },
      ],
      planNeedsAttentionNotice: [
        { planId: "p1", result: planNeedsAttentionNotice("p1") },
      ],
      planSignedOffNotice: [
        {
          args: ["p1", "i1", "branch/x", "abc", 'all "done"', false, []],
          result: planSignedOffNotice(
            "p1",
            "i1",
            "branch/x",
            "abc",
            'all "done"',
            false,
            [],
          ),
        },
        {
          args: ["p1", "i1", "branch/x", null, "s", true, ["r1", "r2"]],
          result: planSignedOffNotice("p1", "i1", "branch/x", null, "s", true, [
            "r1",
            "r2",
          ]),
        },
      ],
      planApprovedNotice: [
        {
          args: ["p1", planBody, null],
          result: planApprovedNotice("p1", planBody, null),
        },
        {
          args: ["p1", planBody, "a note"],
          result: planApprovedNotice("p1", planBody, "a note"),
        },
        {
          args: ["p1", bigBody, null],
          result: planApprovedNotice("p1", bigBody, null),
        },
        {
          args: ["p1", bigBody, "n".repeat(500)],
          result: planApprovedNotice("p1", bigBody, "n".repeat(500)),
        },
      ],
      cancelledReason: [
        {
          args: ["why", "failed", "boom \u001b[31m x"],
          result: cancelledReason("why", {
            state: "failed",
            state_reason: "boom \u001b[31m x",
          }),
        },
        {
          args: ["why", "queued", null],
          result: cancelledReason("why", {
            state: "queued",
            state_reason: null,
          }),
        },
      ],
      findingTask: [
        {
          finding: finding({}),
          attempt: 1,
          evidence: 'ev "q"',
          result: findingTask(finding({}), 1, 'ev "q"'),
        },
      ],
      findingNoticeBody: [
        "raised",
        "resolved",
        "escalated",
        "cancelled",
      ].flatMap((event) =>
        [
          "second_unresolved",
          "timed_out",
          "target_ended",
          "raiser_ended",
          null,
        ].map((reason) => ({
          finding: finding({ state_reason: reason, interventions: 2 }),
          event,
          lastCheck: event === "raised" ? null : 'check "x"',
          result: findingNoticeBody(
            finding({ state_reason: reason, interventions: 2 }),
            event as never,
            event === "raised" ? null : 'check "x"',
          ),
        })),
      ),
      packageReviewedNotice: [
        {
          args: ["p1", "a", "r1", "c1"],
          result: packageReviewedNotice("p1", "a", "r1", "c1"),
        },
      ],
      planCancelledNotice: [
        { args: ["p1", null], result: planCancelledNotice("p1", undefined) },
        { args: ["p1", "a"], result: planCancelledNotice("p1", "a") },
      ],
      integrationConflictNotice: [
        {
          args: ["i1", "r1", ["a.rs", "b/c.rs"], 0],
          result: integrationConflictNotice("i1", "r1", ["a.rs", "b/c.rs"], 0),
        },
        {
          args: ["i1", "r1", ["a.rs"], 7],
          result: integrationConflictNotice("i1", "r1", ["a.rs"], 7),
        },
      ],
      deliveryProblemNotice: [
        {
          args: ["m1", "dev-1", "failed", "boom\u001b[31m", 'first "line"'],
          result: deliveryProblemNotice(
            "m1",
            "dev-1",
            "failed",
            "boom\u001b[31m",
            'first "line"',
          ),
        },
        {
          args: ["m1", "dev-1", "expired", null, "x"],
          result: deliveryProblemNotice("m1", "dev-1", "expired", null, "x"),
        },
      ],
      agentStuckNotice: [
        ["stalled", false],
        ["blocked", false],
        ["blocked", true],
      ].map(([kind, relay]) => ({
        args: [kind, "dev-1", relay],
        result: agentStuckNotice(kind as never, "dev-1", relay as boolean),
      })),
      lostNotice: [
        {
          agent,
          reason: "pane_gone",
          branch: "feat/x",
          ids: [],
          paused: false,
          result: lostNotice(agent, "pane_gone", "feat/x", [], false),
        },
        {
          agent,
          reason: "found_dead_at_start",
          branch: null,
          ids: Array.from({ length: 13 }, (_, i) => `m${i}`),
          paused: true,
          result: lostNotice(
            agent,
            "found_dead_at_start",
            null,
            Array.from({ length: 13 }, (_, i) => `m${i}`),
            true,
          ),
        },
      ],
    }),
  );
  return out;
}

// ------------------------------------------------------------------------------------------------ the export

/** Every exported file by repository-relative path, as the text the exporter writes. */
export async function exportFiles(): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  for (const [name, content] of await sequenceGroups())
    files.set(name, content);
  files.set(`${EXPORT_LAYOUT.common}/core-methods.json`, text(coreMethods()));
  for (const [name, content] of planFixtures()) files.set(name, content);
  for (const [name, content] of textFixtures()) files.set(name, content);
  // `npm run format:check` covers test/, so the fixtures there are written the way prettier writes them.
  for (const [name, content] of files)
    if (name.startsWith("test/"))
      files.set(name, await format(content, { parser: "json" }));
  return files;
}

export function sha256(textValue: string): string {
  return createHash("sha256").update(textValue).digest("hex");
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

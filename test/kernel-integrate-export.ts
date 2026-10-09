/**
 * Writes the fixtures the Rust integration pipeline (rust/crates/kernel/src/integrate.rs) is tested against:
 *
 *  - rust/crates/kernel/tests/parity-integrate/scenarios.json: scenarios that run src/integration.ts (`integrate`,
 *    `settleIntegration`, `recoverIntegrations`) against scratch git repositories under os.tmpdir() and a real
 *    ControllerCore, with the seeded clock and randomness of test/kernel-parity-hooks.ts. A scenario is a list of steps:
 *    controller operations (as in test/kernel-sequences) and `git.*` steps that build or change the repository or run the
 *    pipeline. Every step is recorded with its resolved arguments, its result or error, the logs the pipeline wrote, a
 *    snapshot of the repository (every branch with its commit id, tree id, parents and full message), and the counters;
 *    the scenario ends with a dump of every ledger table (messages and notices included).
 *  - rust/crates/kernel/tests/parity-integrate/squash.json: `squashMessage` over a table of inputs.
 *
 *   npm run build && node dist/test/kernel-integrate-export.js [--out <dir>]
 *
 * Git runs with an empty global configuration, no system configuration, an isolated HOME, a fixed identity and fixed
 * author and committer dates, so commit ids repeat from run to run and between Node and Rust. The files live outside
 * rust/crates/kernel/tests/parity, so the replay of the kernel sequences (tests/replay.rs) does not read them;
 * tests/integrate.rs does.
 */
import { execFileSync } from "node:child_process";
import childProcess from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
import path from "node:path";
import { ControllerCore } from "../src/controller/core.js";
import { openDatabase } from "../src/controller/database.js";
import {
  branchTip,
  commitExists,
  commitSubject,
  coveredReports,
  deleteBranchAt,
  headCommit,
  isInHead,
  mergeIntoBranch,
} from "../src/git.js";
import {
  checkCommitMessage,
  integrationBranchName,
  parseCommitSubject,
  slugify,
  withSuffix,
} from "../src/conventions.js";
import {
  integrate,
  recoverIntegrations,
  settleIntegration,
  squashMessage,
  type IntegrationDeps,
  type IntegrationGit,
} from "../src/integration.js";
import { installHooks, SeededStream } from "./kernel-parity-hooks.js";
import {
  compactSequence,
  dumpTables,
  packedText,
  stable,
  type Tables,
} from "./kernel-parity-export.js";
import { removeTempDir, tempDir } from "./tmp.js";

const root = path.resolve(import.meta.dirname, "..", "..");
export const INTEGRATE_DIRECTORY = "rust/crates/kernel/tests/parity-integrate";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

// ------------------------------------------------------------------------------------------------ git environment

/** The fixed author and committer date of every commit a scenario makes, the pipeline's included. */
const PINNED_DATE = "1767225600 +0000";
const PINNED_ENV: Record<string, string> = {
  GIT_AUTHOR_DATE: PINNED_DATE,
  GIT_COMMITTER_DATE: PINNED_DATE,
};
const OWNER_CREDENTIAL =
  "owner-credential-0123456789-abcdefghijklmnopqrstuvwxyz";
const PROJECT = {
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

type ExecFile = typeof childProcess.execFile;

/** The pipeline's own git runs (src/git.ts) take a cleaned environment; the pinned dates are added to it here. */
function pinPipelineDates(): () => void {
  const original = childProcess.execFile;
  const wrapped = ((
    file: string,
    args: unknown,
    options: unknown,
    ...rest: unknown[]
  ) => {
    const given = (options ?? {}) as { env?: NodeJS.ProcessEnv };
    return (original as unknown as (...a: unknown[]) => unknown)(
      file,
      args,
      { ...given, env: { ...given.env, ...PINNED_ENV } },
      ...rest,
    );
  }) as unknown as ExecFile;
  (childProcess as { execFile: ExecFile }).execFile = wrapped;
  syncBuiltinESMExports();
  return () => {
    (childProcess as { execFile: ExecFile }).execFile = original;
    syncBuiltinESMExports();
  };
}

class Repo {
  readonly root: string;
  readonly #env: NodeJS.ProcessEnv;

  constructor(base: string) {
    this.root = path.join(base, "repo");
    mkdirSync(this.root);
    const home = path.join(base, "home");
    mkdirSync(home);
    this.#env = {
      PATH: process.env.PATH ?? "/usr/bin:/bin",
      HOME: home,
      LC_ALL: "C",
      GIT_CONFIG_NOSYSTEM: "1",
      GIT_CONFIG_GLOBAL: "/dev/null",
      GIT_TERMINAL_PROMPT: "0",
      GIT_AUTHOR_NAME: "scratch",
      GIT_AUTHOR_EMAIL: "scratch@localhost",
      GIT_COMMITTER_NAME: "scratch",
      GIT_COMMITTER_EMAIL: "scratch@localhost",
      ...PINNED_ENV,
    };
  }

  run(args: string[]): { code: number; out: string } {
    try {
      const out = execFileSync("git", ["-C", this.root, ...args], {
        env: this.#env,
        encoding: "utf8",
        stdio: ["ignore", "pipe", "ignore"],
      });
      return { code: 0, out };
    } catch (error) {
      const failure = error as { status?: number | null };
      return { code: failure.status ?? -1, out: "" };
    }
  }

  must(args: string[]): string {
    const outcome = this.run(args);
    if (outcome.code !== 0)
      throw new Error(`git ${args.join(" ")} failed (${outcome.code})`);
    return outcome.out;
  }

  init(): void {
    this.must(["init", "-q", "-b", "main"]);
  }

  commit(spec: {
    branch: string;
    from?: string;
    files: Record<string, string | null>;
    message: string;
  }): string {
    if (spec.from !== undefined)
      this.must(["checkout", "-q", "-B", spec.branch, spec.from]);
    for (const [name, content] of Object.entries(spec.files)) {
      const file = path.join(this.root, name);
      if (content === null) rmSync(file, { force: true });
      else {
        mkdirSync(path.dirname(file), { recursive: true });
        writeFileSync(file, content);
      }
    }
    this.must(["add", "-A"]);
    this.must([
      "commit",
      "-q",
      "--no-verify",
      "--no-gpg-sign",
      "--allow-empty",
      "-m",
      spec.message,
    ]);
    const sha = this.must(["rev-parse", "HEAD"]).trim();
    if (spec.branch !== "main") this.must(["checkout", "-q", "main"]);
    return sha;
  }

  /** Every branch with its commit id, tree id, parents and full message. */
  snapshot(): Json {
    const refs = this.must([
      "for-each-ref",
      "--format=%(refname)",
      "refs/heads",
    ])
      .split("\n")
      .filter((name) => name !== "");
    return {
      head: this.must(["symbolic-ref", "-q", "HEAD"]).trim(),
      refs: refs.map((ref) => {
        const raw = this.must(["cat-file", "commit", ref]);
        const split = raw.indexOf("\n\n");
        const header = raw.slice(0, split).split("\n");
        return {
          ref,
          sha: this.must(["rev-parse", ref]).trim(),
          tree: header.find((l) => l.startsWith("tree "))?.slice(5) ?? "",
          parents: header
            .filter((l) => l.startsWith("parent "))
            .map((l) => l.slice(7)),
          message: raw.slice(split + 2),
        };
      }),
    };
  }
}

// ------------------------------------------------------------------------------------------------ scenarios

export interface Step {
  /** A `ControllerCore` method, `dump`, or `git.init|commit|run|integrate|settle|recover`. */
  readonly op: string;
  readonly context?: { credential: string };
  readonly args?: unknown[];
  readonly as?: string;
  readonly dump?: boolean;
}

export interface Scenario {
  readonly name: string;
  readonly steps: Step[];
}

interface Bindings {
  readonly values: Map<string, unknown>;
  readonly core: () => ControllerCore;
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

function resolve(value: unknown, bindings: Bindings): unknown {
  if (typeof value === "string" && /^\$[A-Za-z]/.test(value)) {
    const [name = "", ...parts] = value.slice(1).split(".");
    if (!bindings.values.has(name))
      throw new Error(`unknown reference ${value}`);
    return lookup(bindings.values.get(name), parts, value);
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

function errorOf(error: unknown): {
  name: string;
  message: string;
  code?: string;
} {
  const e = error as { name?: unknown; message?: unknown; code?: unknown };
  return {
    name: typeof e?.name === "string" ? e.name : "Error",
    message: typeof e?.message === "string" ? e.message : String(error),
    ...(typeof e?.code === "string" ? { code: e.code } : {}),
  };
}

async function runScenario(scenario: Scenario): Promise<Json> {
  const seed = `integrate-${scenario.name}`;
  const directory = tempDir("capstan-kernel-integrate-");
  const hooks = installHooks(seed);
  const unpin = pinPipelineDates();
  const repo = new Repo(directory);
  const stateDirectory = path.join(directory, "state");
  mkdirSync(stateDirectory, { mode: 0o700 });
  let core: ControllerCore | undefined;
  const bindings: Bindings = {
    values: new Map<string, unknown>([
      ["owner", { credential: OWNER_CREDENTIAL }],
      [
        "internal",
        {
          credential: new SeededStream(seed).bytes(32).toString("base64url"),
        },
      ],
    ]),
    core: () => {
      if (core === undefined) throw new Error("no controller is open");
      return core;
    },
  };
  let logs: Array<{ event: string; details: unknown }> = [];
  let contextCounter = 0;
  const git: IntegrationGit = {
    headCommit: () => headCommit(repo.root),
    commitExists: (sha) => commitExists(repo.root, sha),
    commitSubject: (sha) => commitSubject(repo.root, sha),
    merge: (input) => mergeIntoBranch(repo.root, input),
    branchTip: (branch) => branchTip(repo.root, branch),
    isInHead: (sha) => isInHead(repo.root, sha),
    deleteBranch: (branch, sha) => deleteBranchAt(repo.root, branch, sha),
    coveredReports: (head, reports, options) =>
      coveredReports(repo.root, head, reports, options),
  };
  const deps = (): IntegrationDeps => ({
    core: bindings.core(),
    git,
    credential: OWNER_CREDENTIAL,
    context: (credential) => {
      contextCounter += 1;
      return {
        credential,
        requestId: `int-req-${contextCounter}`,
        idempotencyKey: `int-idem-${contextCounter}`,
        expectedVersion: bindings.core().stateVersion,
        inputRevision: bindings.core().inputRevision,
      };
    },
    log: (event, details) => {
      logs.push({ event, details });
    },
  });

  const records: Json[] = [];
  const steps: Step[] = [{ op: "open" }, ...scenario.steps];
  try {
    for (const step of steps) {
      const record: { [key: string]: Json } = { op: step.op };
      let result: unknown = null;
      let failure: { name: string; message: string; code?: string } | undefined;
      logs = [];
      try {
        if (step.op === "open") {
          const project = resolve(PROJECT, bindings) as Record<string, unknown>;
          record.args = stable([{ project }]);
          // The ledger's migration timestamps come from the system clock in both implementations.
          (
            await openDatabase(path.join(stateDirectory, "controller.sqlite"))
          ).close();
          core = await hooks.run(() =>
            ControllerCore.open({ stateDirectory, project: project as never }),
          );
        } else if (step.op === "dump") {
          record.args = [];
        } else if (step.op.startsWith("git.")) {
          const args = resolve(step.args ?? [], bindings) as Array<
            Record<string, unknown>
          >;
          record.args = stable(args);
          const arg = args[0] ?? {};
          if (step.op === "git.init") {
            repo.init();
          } else if (step.op === "git.commit") {
            result = repo.commit(arg as never);
          } else if (step.op === "git.run") {
            result = { code: repo.run(arg.args as string[]).code };
          } else if (step.op === "git.integrate") {
            result = await hooks.run(() =>
              integrate(deps(), {
                reportIds: arg.reportIds as string[],
                requestedBy: arg.requestedBy as string,
              }),
            );
          } else if (step.op === "git.settle") {
            result = await hooks.run(() =>
              settleIntegration(deps(), {
                integrationId: arg.integrationId as string,
                outcome: arg.outcome as "confirmed" | "discarded",
              }),
            );
          } else if (step.op === "git.recover") {
            result = await hooks.run(() =>
              recoverIntegrations(deps(), arg.scope as "pending" | "all"),
            );
          } else throw new Error(`unknown step ${step.op}`);
        } else {
          const args: unknown[] = [];
          if (step.context !== undefined) {
            const spec = resolve(step.context, bindings) as {
              credential: string;
            };
            const made = {
              credential: spec.credential,
              requestId: `req-${records.length}`,
              idempotencyKey: `idem-${records.length}`,
              expectedVersion: bindings.core().stateVersion,
              inputRevision: bindings.core().inputRevision,
            };
            record.context = stable(made);
            args.push(made);
          }
          const rest = resolve(step.args ?? [], bindings) as unknown[];
          args.push(...rest);
          record.args = stable(rest);
          const target = bindings.core();
          const member = (target as unknown as Record<string, unknown>)[
            step.op
          ];
          const raw: unknown = hooks.run(() =>
            typeof member === "function"
              ? (member as (...a: unknown[]) => unknown).apply(target, args)
              : member,
          );
          result = raw;
        }
        result = stable(
          JSON.parse(JSON.stringify(result === undefined ? null : result)),
        );
        if (step.as !== undefined) bindings.values.set(step.as, result);
      } catch (error) {
        failure = errorOf(error);
      }
      if (failure === undefined) record.result = stable(result);
      else {
        record.error = failure.name;
        record.message = failure.message;
        if (failure.code !== undefined) record.code = failure.code;
      }
      if (step.as !== undefined) record.as = step.as;
      if (step.op.startsWith("git.") && step.op !== "git.init") {
        record.repo = repo.snapshot();
        if (step.op !== "git.commit" && step.op !== "git.run")
          record.logs = stable(logs);
      }
      if (core !== undefined) {
        record.stateVersion = core.stateVersion;
        record.inputRevision = core.inputRevision;
      }
      if (step.dump === true || step.op === "dump")
        record.tables = dumpTables(
          path.join(stateDirectory, "controller.sqlite"),
        );
      records.push(record);
    }
    return stable({
      name: scenario.name,
      seed,
      requires: [],
      project: resolve(PROJECT, bindings) as never,
      gitEnv: PINNED_ENV,
      steps: records,
      tables: dumpTables(path.join(stateDirectory, "controller.sqlite")),
    });
  } finally {
    try {
      core?.close();
    } catch {
      // already closed
    }
    unpin();
    hooks.restore();
    removeTempDir(directory);
  }
}

// ------------------------------------------------------------------------------------------------ step builders

const cred = (key: string): { credential: string } => ({
  credential: `$${key}.credential`,
});
const OWNER = cred("owner");
const key = (id: string): string => id.replace(/-/g, "");

const ROLES = [
  {
    name: "pm",
    kind: "PM",
    host: "claude",
    configHash: "b".repeat(64),
  },
  {
    name: "developer",
    kind: "Developer",
    host: "claude",
    configHash: "a".repeat(64),
  },
  {
    name: "reviewer",
    kind: "Verifier",
    host: "claude",
    configHash: "c".repeat(64),
  },
];

function member(id: string, role: "PM" | "Developer" | "Verifier"): Step[] {
  const roleName =
    role === "PM" ? "pm" : role === "Developer" ? "developer" : "reviewer";
  const k = key(id);
  const steps: Step[] = [
    {
      op: "createSeat",
      context: OWNER,
      args: [{ seatId: `seat-${id}`, name: `seat-${id}`, role }],
    },
    {
      op: "createActor",
      context: OWNER,
      args: [{ displayName: id, role, seatId: `seat-${id}` }],
      as: k,
    },
    {
      op: "registerAgent",
      context: OWNER,
      args: [
        {
          agentId: id,
          roleName,
          seatId: `seat-${id}`,
          actorId: `$${k}.actorId`,
        },
      ],
    },
  ];
  if (role !== "PM")
    steps.push({
      op: "recordAgentPane",
      context: cred("internal"),
      args: [
        {
          agentId: id,
          workspaceId: "ws-1",
          paneId: `pane-${id}`,
          worktreePath: `/tmp/wt/${id}`,
          branch: `feat/${id}`,
          baseSha: "$base",
        },
      ],
    });
  return steps;
}

/** The repository's first commit, and a team: a PM, the named developers and reviewers. */
function team(
  base: Record<string, string>,
  devs: string[],
  reviewers: string[] = ["rev-1"],
  extra: string[] = [],
): Step[] {
  return [
    { op: "git.init" },
    {
      op: "git.commit",
      args: [{ branch: "main", files: base, message: "chore: base" }],
      as: "base",
    },
    { op: "syncRoleDefinitions", context: OWNER, args: [ROLES] },
    ...member("pm-1", "PM"),
    ...extra.flatMap((id) => member(id, "Developer")),
    ...devs.flatMap((id) => member(id, "Developer")),
    ...reviewers.flatMap((id) => member(id, "Verifier")),
  ];
}

function commit(
  as: string,
  branch: string,
  files: Record<string, string | null>,
  message: string,
  from: string = "$base",
): Step {
  return {
    op: "git.commit",
    args: [{ branch, from, files, message }],
    as,
  };
}

function report(
  dev: string,
  sha: string,
  summary: string,
  as: string,
  extra: Record<string, unknown> = {},
): Step {
  return {
    op: "recordAgentReport",
    context: cred(key(dev)),
    args: [
      {
        commitSha: sha,
        summary,
        evidence: {
          baseSha: "$base",
          branch: `feat/${dev}`,
          branchTip: sha,
          checkedAt: "2026-01-01T00:00:00.000Z",
          commitExists: true,
          generation: 1,
          isAncestorOfBase: false,
          isAncestorOfTip: true,
          ...extra,
        },
      },
    ],
    as,
  };
}

function review(
  subject: string,
  reviewer: string,
  verdict: "pass" | "findings" = "pass",
): Step[] {
  return [
    {
      op: "beginReview",
      context: cred("pm1"),
      args: [
        {
          subjectId: subject,
          reviewerRole: "reviewer",
          reviewerAgentId: reviewer,
        },
      ],
    },
    {
      op: "completeReview",
      context: cred(key(reviewer)),
      args: [
        { verdict, text: `Looks ${verdict === "pass" ? "fine" : "wrong"}.` },
      ],
    },
  ];
}

/** The pipeline run on reports (`integrate` of src/integration.ts), bound to `as`. */
const integrateStep = (
  reportIds: string[],
  as: string,
  requestedBy = "pm",
): Step => ({
  op: "git.integrate",
  args: [{ reportIds, requestedBy }],
  as,
});

const settleStep = (
  integration: string,
  outcome: "confirmed" | "discarded",
): Step => ({
  op: "git.settle",
  args: [{ integrationId: `$${integration}.integrationId`, outcome }],
});

const rid = (as: string): string => `$${as}.record.reportId`;
const gitRun = (...args: string[]): Step => ({
  op: "git.run",
  args: [{ args }],
});
const DUMP: Step = { op: "dump" };

const PLAN_BODY = JSON.stringify({
  summary: "Build it",
  packages: [
    {
      id: "core",
      title: "Core",
      owns: ["src/core"],
      interfaces: ["core api"],
      dependsOn: [],
      estimateHours: 2,
      acceptance: ["core works"],
      risks: ["none"],
      type: "feat",
      scope: "core",
    },
    {
      id: "docs",
      title: "Docs",
      owns: ["src/docs"],
      interfaces: ["docs api"],
      dependsOn: [],
      estimateHours: 2,
      acceptance: ["docs works"],
      risks: ["none"],
      type: "docs",
    },
    {
      id: "ui",
      title: "UI",
      owns: ["src/ui"],
      interfaces: ["ui api"],
      dependsOn: [],
      estimateHours: 2,
      acceptance: ["ui works"],
      risks: ["none"],
      type: "fix",
      scope: "ui",
      breaking: true,
    },
  ],
  integrationOrder: ["core", "docs", "ui"],
});

function planSteps(
  title: string,
  assignments: Array<[string, string]>,
): Step[] {
  return [
    {
      op: "openPlan",
      context: cred("pm1"),
      args: [{ tier: "normal", title }],
      as: "plan",
    },
    {
      op: "submitPlan",
      context: cred("arch1"),
      args: [
        {
          planId: "plan-1",
          bodyJson: PLAN_BODY,
          baseSha: "$base",
          review: true,
        },
      ],
    },
    ...review("plan-1", "rev-1"),
    ...assignments.map(([packageId, agentId]): Step => ({
      op: "assignPackage",
      context: cred("pm1"),
      args: [{ planId: "plan-1", packageId, agentId }],
    })),
  ];
}

const BASE_FILES = { "shared.txt": "base\n", "keep.txt": "keep\n" };

function scenarios(): Scenario[] {
  const out: Scenario[] = [];

  // Two clean reports, merged, reviewed, merged into the base by the operator, then confirmed; a third report that is
  // an ancestor of a member commit is covered.
  out.push({
    name: "clean-merge",
    steps: [
      ...team(BASE_FILES, ["dev-1", "dev-2", "dev-3"]),
      commit("c3", "w3", { "d.txt": "d\n" }, "feat(core): add d"),
      commit("c1", "w1", { "b.txt": "b\n" }, "feat(core): add b", "$c3"),
      commit("c2", "w2", { "c.txt": "c\n" }, "fix: add c"),
      report("dev-3", "$c3", "Added d", "r3"),
      report("dev-1", "$c1", "Added b on top of d", "r1"),
      report("dev-2", "$c2", "Added c", "r2"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      integrateStep([rid("r1"), rid("r2")], "int1"),
      DUMP,
      ...review("$int1.integrationId", "rev-1"),
      {
        op: "git.settle",
        args: [{ integrationId: "$int1.integrationId", outcome: "confirmed" }],
        as: "settleEarly",
      },
      gitRun("merge", "-q", "--ff-only", "$int1.branch"),
      settleStep("int1", "confirmed"),
      {
        op: "coverageCandidates",
        args: ["$owner.credential", "$int1.integrationId"],
      },
      { op: "integrations", args: ["$owner.credential"] },
    ],
  });

  // A conflict between two reports: reported with its files, a notice to the PM for an operator run, nothing left behind;
  // then the first report alone merges and is discarded.
  out.push({
    name: "conflict",
    steps: [
      ...team(BASE_FILES, ["dev-1", "dev-2"]),
      commit("c1", "w1", { "shared.txt": "one\n" }, "feat: one"),
      commit("c2", "w2", { "shared.txt": "two\n" }, "feat: two"),
      report("dev-1", "$c1", "One", "r1"),
      report("dev-2", "$c2", "Two", "r2"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      integrateStep([rid("r1"), rid("r2")], "int1", "operator"),
      integrateStep([rid("r2"), rid("r1")], "int2", "pm"),
      integrateStep([rid("r1")], "int3"),
      settleStep("int3", "discarded"),
      settleStep("int1", "discarded"),
      { op: "integrations", args: ["$owner.credential"] },
    ],
  });

  // Conflicting paths that need escaping, and one that is too long for the record.
  const awkward: Record<string, string | null> = {
    "sp ace.txt": "base\n",
    'qu"ote.txt': "base\n",
    "co,mma#hash.txt": "base\n",
    "é.txt": "base\n",
    [`${"d".repeat(100)}/${"f".repeat(120)}`]: "base\n",
  };
  const edit = (value: string): Record<string, string> =>
    Object.fromEntries(Object.keys(awkward).map((name) => [name, value]));
  out.push({
    name: "conflict-paths",
    steps: [
      ...team({ ...BASE_FILES, ...(awkward as Record<string, string>) }, [
        "dev-1",
        "dev-2",
      ]),
      commit("c1", "w1", edit("one\n"), "feat: one"),
      commit("c2", "w2", edit("two\n"), "feat: two"),
      report("dev-1", "$c1", "One", "r1"),
      report("dev-2", "$c2", "Two", "r2"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      integrateStep([rid("r1"), rid("r2")], "int1", "operator"),
    ],
  });

  // More conflicting files than a record keeps.
  const many = Object.fromEntries(
    Array.from({ length: 53 }, (_, i) => [
      `dir/file-${String(i).padStart(2, "0")}.txt`,
      "base\n",
    ]),
  );
  const manyEdit = (value: string): Record<string, string> =>
    Object.fromEntries(Object.keys(many).map((name) => [name, value]));
  out.push({
    name: "conflict-many",
    steps: [
      ...team({ ...BASE_FILES, ...many }, ["dev-1", "dev-2"]),
      commit("c1", "w1", manyEdit("one\n"), "feat: one"),
      commit("c2", "w2", manyEdit("two\n"), "feat: two"),
      report("dev-1", "$c1", "One", "r1"),
      report("dev-2", "$c2", "Two", "r2"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      integrateStep([rid("r1"), rid("r2")], "int1", "operator"),
    ],
  });

  // A report already contained in the base (the operator merged its branch meanwhile), and a report whose commit does not
  // exist.
  out.push({
    name: "contained-and-missing",
    steps: [
      ...team(BASE_FILES, ["dev-1", "dev-2"]),
      commit("c1", "w1", { "a.txt": "a\n" }, "feat: add a"),
      commit("c2", "w2", { "c.txt": "c\n" }, "feat: add c"),
      report("dev-1", "$c1", "Added a", "r1"),
      report("dev-2", "$c2", "Added c", "r2"),
      report("dev-2", "0000000000000000000000000000000000000abc", "Gone", "r3"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      ...review(rid("r3"), "rev-1"),
      gitRun("merge", "-q", "--ff-only", "w1"),
      integrateStep([rid("r1")], "int1"),
      integrateStep([rid("r3")], "int2"),
      integrateStep([rid("r2"), rid("r1")], "int3"),
      { op: "integrations", args: ["$owner.credential"] },
    ],
  });

  // An integration the daemon left running with its branch: the next integrate recovers it first.
  out.push({
    name: "recover-interrupted",
    steps: [
      ...team(BASE_FILES, ["dev-1", "dev-2"]),
      commit("c1", "w1", { "b.txt": "b\n" }, "feat: add b"),
      commit("c2", "w2", { "c.txt": "c\n" }, "feat: add c"),
      report("dev-1", "$c1", "Added b", "r1"),
      report("dev-2", "$c2", "Added c", "r2"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      {
        op: "beginIntegration",
        context: OWNER,
        args: [
          {
            integrationId: "int-cut",
            reportIds: [rid("r1")],
            baseSha: "$base",
            branch: "integration/cut",
            requestedBy: "pm",
          },
        ],
      },
      gitRun("branch", "integration/cut", "$c1"),
      { op: "git.recover", args: [{ scope: "all" }] },
      {
        op: "beginIntegration",
        context: OWNER,
        args: [
          {
            integrationId: "int-cut-2",
            reportIds: [rid("r1")],
            baseSha: "$base",
            branch: "integration/cut-2",
            requestedBy: "pm",
          },
        ],
      },
      integrateStep([rid("r2")], "int1"),
      { op: "runningIntegrations", args: ["$owner.credential"] },
      { op: "git.recover", args: [{ scope: "all" }] },
      { op: "integrations", args: ["$owner.credential"] },
    ],
  });

  // Settling: not merged, not in HEAD, a branch that cannot be deleted while it is checked out and is swept later.
  out.push({
    name: "settle",
    steps: [
      ...team(BASE_FILES, ["dev-1", "dev-2"]),
      commit("c1", "w1", { "b.txt": "b\n" }, "feat: add b"),
      commit("c2", "w2", { "shared.txt": "other\n" }, "feat: other"),
      commit("c3", "w3", { "shared.txt": "third\n" }, "feat: third"),
      report("dev-1", "$c1", "Added b", "r1"),
      report("dev-2", "$c2", "Other", "r2"),
      report("dev-2", "$c3", "Third", "r3"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      ...review(rid("r3"), "rev-1"),
      integrateStep([rid("r2"), rid("r3")], "failed"),
      settleStep("failed", "confirmed"),
      integrateStep([rid("r1")], "int1"),
      ...review("$int1.integrationId", "rev-1"),
      settleStep("int1", "confirmed"),
      gitRun("checkout", "-q", "$int1.branch"),
      settleStep("int1", "discarded"),
      {
        op: "git.settle",
        args: [{ integrationId: "no-such-integration", outcome: "discarded" }],
      },
      gitRun("checkout", "-q", "main"),
      { op: "git.recover", args: [{ scope: "pending" }] },
      { op: "git.recover", args: [{ scope: "all" }] },
      { op: "integrations", args: ["$owner.credential"] },
    ],
  });

  // A confirmed integration branch that cannot be deleted at settle time, swept by the daemon-start recovery.
  out.push({
    name: "sweep-at-start",
    steps: [
      ...team(BASE_FILES, ["dev-1"]),
      commit("c1", "w1", { "b.txt": "b\n" }, "feat: add b"),
      report("dev-1", "$c1", "Added b", "r1"),
      ...review(rid("r1"), "rev-1"),
      integrateStep([rid("r1")], "int1"),
      gitRun("checkout", "-q", "$int1.branch"),
      settleStep("int1", "discarded"),
      gitRun("checkout", "-q", "main"),
      { op: "git.recover", args: [{ scope: "all" }] },
      { op: "git.recover", args: [{ scope: "pending" }] },
    ],
  });

  // Names of the squash commit and branch from a plan: type, scope, breaking and the title; and a taken branch name.
  out.push({
    name: "plan-naming",
    steps: [
      ...team(BASE_FILES, ["dev-1", "dev-2"], ["rev-1"], ["arch-1"]),
      commit("c1", "w1", { "core.txt": "core\n" }, "wip"),
      commit("c2", "w2", { "docs.txt": "docs\n" }, "docs: write"),
      ...planSteps("feat: Build the thing", [
        ["core", "dev-1"],
        ["docs", "dev-2"],
      ]),
      report("dev-1", "$c1", "Core done with a second line", "r1"),
      report("dev-2", "$c2", "Docs written", "r2"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      gitRun("branch", "integration/plan-1-build-the-thing", "$base"),
      integrateStep([rid("r1"), rid("r2")], "int1"),
      settleStep("int1", "discarded"),
      integrateStep([rid("r1"), rid("r2")], "int2"),
    ],
  });

  // A plan title with letters that normalise (NFKD) or fold, in the branch name and the subject.
  out.push({
    name: "plan-unicode",
    steps: [
      ...team(BASE_FILES, ["dev-1"], ["rev-1"], ["arch-1"]),
      commit("c1", "w1", { "core.txt": "core\n" }, "wip"),
      ...planSteps("feat: Zażółć Éclair ß ﬁne ² naïve Ærø", [
        ["core", "dev-1"],
      ]),
      report("dev-1", "$c1", "Core done", "r1"),
      ...review(rid("r1"), "rev-1"),
      integrateStep([rid("r1")], "int1"),
    ],
  });

  // Without a plan the subject comes from the reports' own commit subjects.
  out.push({
    name: "commit-subjects",
    steps: [
      ...team(BASE_FILES, ["dev-1", "dev-2", "dev-3"]),
      commit(
        "c1",
        "w1",
        { "a.txt": "a\n" },
        "refactor(parser)!: split the lexer",
      ),
      commit("c2", "w2", { "b.txt": "b\n" }, "fix(parser): handle empty input"),
      commit("c3", "w3", { "c.txt": "c\n" }, "just some words"),
      report("dev-1", "$c1", "Split", "r1"),
      report("dev-2", "$c2", "Fixed", "r2"),
      report("dev-3", "$c3", "chore(deps): Bumped things and more", "r3"),
      ...review(rid("r1"), "rev-1"),
      ...review(rid("r2"), "rev-1"),
      ...review(rid("r3"), "rev-1"),
      integrateStep([rid("r1"), rid("r2")], "int1"),
      settleStep("int1", "discarded"),
      integrateStep([rid("r3")], "int2"),
      settleStep("int2", "discarded"),
      integrateStep([rid("r2"), rid("r3")], "int3"),
    ],
  });

  // Coverage of reports an integration did not merge: by tree, by merge, by a line-wise match, not covered, and by an
  // earlier integration whose head a member commit builds on.
  const lines = "one\ntwo\nthree four five six\nseven\neight\nnine\nten\n";
  out.push({
    name: "coverage",
    steps: [
      ...team(
        {
          ...BASE_FILES,
          "x.txt": "x-old\n",
          "y.txt": "y1\ny2\ny3\ny4\ny5\ny6\ny7\ny8\n",
          "z.txt": lines,
          "q.txt": "q-old\n",
        },
        ["dev-1", "dev-2", "dev-3", "dev-4", "dev-5", "dev-6"],
      ),
      commit(
        "cA",
        "wa",
        {
          "a.txt": "a\n",
          "x.txt": "x-new\n",
          "y.txt": "Y1\ny2\ny3\ny4\ny5\ny6\ny7\nY8\n",
          "z.txt": lines.replace(
            "three four five six",
            "three four five six seven eight",
          ),
        },
        "feat: integrated work",
      ),
      commit("cT", "wt", { "x.txt": "x-new\n" }, "feat: tree covered"),
      commit(
        "cM",
        "wm",
        { "y.txt": "Y1\ny2\ny3\ny4\ny5\ny6\ny7\ny8\n" },
        "feat: merge covered",
      ),
      commit(
        "cL",
        "wl",
        {
          "z.txt": lines.replace(
            "three four five six",
            "three four five six seven",
          ),
        },
        "feat: line covered",
      ),
      commit("cN", "wn", { "q.txt": "q-new\n" }, "feat: not covered"),
      commit("cI", "wi", { "i.txt": "iii-line one\n" }, "feat: held earlier"),
      report("dev-1", "$cA", "Integrated work", "rA"),
      report("dev-2", "$cT", "Tree covered", "rT"),
      report("dev-3", "$cM", "Merge covered", "rM"),
      report("dev-4", "$cL", "Line covered", "rL"),
      report("dev-5", "$cN", "Not covered", "rN"),
      report("dev-6", "$cI", "Held earlier", "rI"),
      ...review(rid("rA"), "rev-1"),
      ...review(rid("rI"), "rev-1"),
      integrateStep([rid("rI")], "earlier"),
      settleStep("earlier", "discarded"),
      commit(
        "cB",
        "wb",
        { "b.txt": "b\n", "i.txt": "totally different text\n" },
        "feat: builds on the earlier head",
        "$earlier.headSha",
      ),
      report("dev-1", "$cB", "Builds on the earlier head", "rB"),
      ...review(rid("rB"), "rev-1"),
      integrateStep([rid("rA"), rid("rB")], "int1"),
      ...review("$int1.integrationId", "rev-1"),
      gitRun("merge", "-q", "--ff-only", "$int1.branch"),
      {
        op: "coverageCandidates",
        args: ["$owner.credential", "$int1.integrationId"],
      },
      settleStep("int1", "confirmed"),
      {
        op: "coverageCandidates",
        args: ["$owner.credential", "$int1.integrationId"],
      },
      { op: "git.recover", args: [{ scope: "all" }] },
    ],
  });

  return out;
}

// ------------------------------------------------------------------------------------------------ squash messages

type Info = Parameters<typeof squashMessage>[0];

function infoOf(
  reports: Array<[string, string, string]>,
  options: {
    planId?: string;
    planTitle?: string;
    packages?: Array<{
      packageId: string;
      type: string | null;
      scope: string | null;
      breaking: boolean;
    }>;
  } = {},
): Info {
  return {
    planId: options.planId ?? null,
    planTitle: options.planTitle ?? null,
    packages: options.packages ?? [],
    reports: reports.map(([reportId, agentId, summary]) => ({
      reportId,
      agentId,
      summary,
      branch: `feat/${agentId}`,
    })),
  } as unknown as Info;
}

function squashCases(): Array<{
  name: string;
  info: Info;
  subjects: Record<string, string>;
}> {
  const pkg = (
    packageId: string,
    type: string | null,
    scope: string | null = null,
    breaking = false,
  ): {
    packageId: string;
    type: string | null;
    scope: string | null;
    breaking: boolean;
  } => ({
    packageId,
    type,
    scope,
    breaking,
  });
  const long = "word ".repeat(120).trim();
  return [
    {
      name: "single-summary",
      info: infoOf([["r1", "dev-1", "Add the parser"]]),
      subjects: {},
    },
    {
      name: "summary-with-type",
      info: infoOf([["r1", "dev-1", "fix(parser): handle empty input"]]),
      subjects: {},
    },
    {
      name: "summary-with-docs-type",
      info: infoOf([["r1", "dev-1", "docs: describe it"]]),
      subjects: {},
    },
    {
      name: "summary-unknown-type",
      info: infoOf([["r1", "dev-1", "wibble: something"]]),
      subjects: {},
    },
    { name: "no-reports", info: infoOf([]), subjects: {} },
    {
      name: "blank-summary",
      info: infoOf([["r1", "dev-1", "   \n  "]]),
      subjects: {},
    },
    {
      name: "second-line-summary",
      info: infoOf([["r1", "dev-1", "\n\n  Second line wins\nthird"]]),
      subjects: {},
    },
    {
      name: "crlf-summary",
      info: infoOf([["r1", "dev-1", "First\r\nSecond"]]),
      subjects: {},
    },
    {
      name: "long-description",
      info: infoOf([["r1", "dev-1", long]]),
      subjects: {},
    },
    {
      name: "long-summary-body",
      info: infoOf([
        ["r1", "dev-1", long],
        ["r2", "dev-2", "x".repeat(900)],
      ]),
      subjects: {},
    },
    {
      name: "unicode-summary",
      info: infoOf([
        [
          "r1",
          "dev-1",
          "Zażółć gęślą jaźń 🙂 日本語のテスト and more words to cut",
        ],
      ]),
      subjects: {},
    },
    {
      name: "co-author-line",
      info: infoOf([
        ["r1", "dev-1", "Done\nCo-Authored-By: Claude <noreply@anthropic.com>"],
        ["r2", "dev-2", "Fine"],
      ]),
      subjects: {},
    },
    {
      name: "co-author-inline",
      info: infoOf([["r1", "dev-1", "Done. Co-authored-by: someone else"]]),
      subjects: {},
    },
    {
      name: "session-line",
      info: infoOf([["r1", "dev-1", "Claude-Session: abc"]]),
      subjects: {},
    },
    {
      name: "footer-line",
      info: infoOf([["r1", "dev-1", "Generated with [Claude Code] done"]]),
      subjects: {},
    },
    { name: "robot", info: infoOf([["r1", "dev-1", "done 🤖"]]), subjects: {} },
    {
      name: "plan-title",
      info: infoOf(
        [
          ["r1", "dev-1", "Core"],
          ["r2", "dev-2", "Docs"],
        ],
        {
          planId: "plan-1",
          planTitle: "Build the thing",
          packages: [pkg("core", "feat", "core"), pkg("docs", "docs")],
        },
      ),
      subjects: {},
    },
    {
      name: "plan-title-typed",
      info: infoOf([["r1", "dev-1", "Core"]], {
        planId: "plan-2",
        planTitle: "fix(core)!: Repair the thing",
        packages: [pkg("core", null, null)],
      }),
      subjects: {},
    },
    {
      name: "plan-fix-and-docs",
      info: infoOf(
        [
          ["r1", "dev-1", "A"],
          ["r2", "dev-2", "B"],
        ],
        {
          planId: "plan-3",
          planTitle: "Tidy up",
          packages: [pkg("a", "docs", "docs"), pkg("b", "fix", "ui", true)],
        },
      ),
      subjects: {},
    },
    {
      name: "plan-two-scopes",
      info: infoOf(
        [
          ["r1", "dev-1", "A"],
          ["r2", "dev-2", "B"],
        ],
        {
          planId: "plan-4",
          planTitle: "Two scopes",
          packages: [pkg("a", "feat", "one"), pkg("b", "feat", "two")],
        },
      ),
      subjects: {},
    },
    {
      name: "plan-untyped",
      info: infoOf([["r1", "dev-1", "A"]], {
        planId: "plan-5",
        planTitle: "refactor: Untyped package",
        packages: [pkg("a", null)],
      }),
      subjects: {},
    },
    {
      name: "plan-untyped-unknown-leading",
      info: infoOf([["r1", "dev-1", "A"]], {
        planId: "plan-6",
        planTitle: "wibble: Untyped package",
        packages: [pkg("a", null)],
      }),
      subjects: {},
    },
    {
      name: "plan-long-title",
      info: infoOf([["r1", "dev-1", "A"]], {
        planId: "plan-7",
        planTitle: `feat(core): ${long}`,
        packages: [pkg("a", "feat", "a-rather-long-scope-name-for-testing")],
      }),
      subjects: {},
    },
    {
      name: "plan-empty-title",
      info: infoOf([["r1", "dev-1", "A"]], {
        planId: "plan-8",
        planTitle: "fix:",
        packages: [],
      }),
      subjects: {},
    },
    {
      name: "own-subjects",
      info: infoOf([
        ["r1", "dev-1", "One"],
        ["r2", "dev-2", "Two"],
        ["r3", "dev-3", "Three"],
      ]),
      subjects: {
        r1: "refactor(parser)!: split the lexer",
        r2: "fix(parser): handle empty input",
        r3: "chore: tidy",
      },
    },
    {
      name: "own-subjects-two-scopes",
      info: infoOf([
        ["r1", "dev-1", "One"],
        ["r2", "dev-2", "Two"],
      ]),
      subjects: { r1: "feat(a): first", r2: "feat(b): second" },
    },
    {
      name: "own-subjects-some-invalid",
      info: infoOf([
        ["r1", "dev-1", "One"],
        ["r2", "dev-2", "Two"],
      ]),
      subjects: { r1: "not conventional", r2: "docs(readme): explain" },
    },
    {
      name: "own-subjects-none-valid",
      info: infoOf([["r1", "dev-1", "Summary text here"]]),
      subjects: { r1: "Feat: capital", r2: "feat(): empty scope" },
    },
    {
      name: "own-subjects-space-description",
      info: infoOf([["r1", "dev-1", "Summary text here"]]),
      subjects: { r1: "feat:  two spaces", r2: "feat(a b): space in scope" },
    },
    {
      name: "own-subjects-ignored-with-plan",
      info: infoOf([["r1", "dev-1", "One"]], {
        planId: "plan-9",
        planTitle: "Plan wins",
        packages: [pkg("a", "docs")],
      }),
      subjects: { r1: "fix: not used" },
    },
  ];
}

// ------------------------------------------------------------------------------------------------ naming conventions

/** Titles for `slugify`: accents, ligatures, compatibility forms, full-width letters, CJK, emoji, combining marks. */
const SLUG_TITLES: string[] = [
  "Build the thing",
  "  --Leading and trailing--  ",
  "UPPER lower 123",
  "under_score and.dot/slash",
  "Zażółć gęślą jaźń",
  "Crème brûlée à la façon",
  "Ångström Ünïcödé Ñandú",
  "Straße ẞ ß",
  "Ærø Øresund Œuvre æther œuf",
  "Đà Nẵng đường",
  "Łódź łatwo",
  "Þor and Ðurđa þðÞÐ",
  "ﬁne ﬂow ﬃ ﬆ ĳ Ĳ ǆ ǅ Ǆ",
  "ＡＢＣ ａｂｃ １２３",
  "ⅩⅡ Ⅷ ⅷ ① ② ⒜",
  "x² y₃ ¼ ½ ¾ 1⁄2",
  "℃ ℉ ㎏ ㎡ ㈱ ㌔",
  "ﷺ ﷲ",
  "日本語のテスト",
  "한국어 테스트 각 힣",
  "العربية עברית",
  "Привіт мир й ё Й Ё",
  "Ελληνικά ΐ ά Σίσυφος ΑΣ",
  "Türkçe İstanbul ı I",
  "emoji 🙂 and 👍🏽 and 🇯🇵",
  "e\u0301 a\u0308 o\u0302\u0323 combining",
  "Việt Nam ếệ Ế Ệ",
  "ǎǐǒǔ ǖǘǚǜ Ǎ",
  "ṩ ḉ ǭ ȫ ǻ",
  "ŉ ſ ʼn",
  "\u200b zero\u00a0width\u3000space",
  "tab\tnew\nline",
  "日本語",
  "",
  "word ".repeat(30),
  "ÀÁÂÃÄÅ".repeat(10),
  "ａ".repeat(60),
  "Pokémon: Pikachu & Raichu #025",
  "ᴀʙᴄ ⓐⓑⓒ ⒶⒷⒸ",
  "ꜰ ꜱ ﬅ ﬓ",
];

function conventionCases(): Json {
  const ids = [
    "plan-1",
    "a_b.c",
    "x".repeat(60),
    "--lead--",
    "",
    "Ünï-cöde",
    "7e16d310-1ab4-42fb-bee9-62929e2cb7e2",
  ];
  const titles = [
    undefined,
    "",
    "Build the thing",
    "Zażółć Éclair ß",
    "ﬁne ½ ＡＢＣ",
    "word ".repeat(30),
    "日本語",
    "!!!",
  ];
  const branches: Json[] = [];
  for (const integrationId of ids)
    for (const planTitle of titles)
      branches.push({
        integrationId,
        planTitle: planTitle ?? null,
        name: integrationBranchName({
          integrationId,
          ...(planTitle === undefined ? {} : { planTitle }),
        }),
      });
  const suffixBranches = [
    "integration/plan-1",
    `integration/${"a".repeat(95)}`,
    `integration/${"a".repeat(88)}-/.`,
    `integration/${"b".repeat(86)}.-b`,
  ];
  const suffixes: Json[] = [];
  for (const branch of suffixBranches)
    for (const n of [1, 2, 3, 10, 99, 999])
      suffixes.push({ branch, n, name: withSuffix(branch, n) });
  const subjects = [
    "feat: add it",
    "feat(core): add it",
    "feat(core)!: add it",
    "feat!: add it",
    "fix(a.b/c_d-e): x",
    "revert: undo",
    "style: pretty",
    "wibble: unknown type",
    "Feat: capital",
    "feat:no space",
    "feat:  two spaces",
    "feat: ",
    "feat:",
    "feat: \t",
    "feat(): empty scope",
    "feat(a b): space in scope",
    "feat(a(b)): nested",
    "feat(a)(b): twice",
    "feat (a): space before",
    "feat(a)): extra",
    ": nothing",
    "",
    "chore: tidy\u2028up",
    "chore: caf\u00e9 \u65e5\u672c",
    "chore: a\rb",
    "docs(readme): explain: more",
    "build!: break",
    "perf(db): faster ",
    "ci: \u00a0nbsp start",
    "test:\u00a0x",
  ];
  const messages = [
    "chore: x\n\nbody",
    "chore: x\nbody without blank",
    "chore: x\n  \nbody",
    "chore: x\r\n\r\nbody",
    "chore: x\r\rbody",
    "not conventional\n\nbody",
    "wip: x\n\nbody",
    "chore(): x\n\nbody",
    "chore:  x\n\nbody",
    "chore: x\n\nCo-Authored-By: Claude <noreply@anthropic.com>",
    "chore: x\n\n  co-authored-by: Some Claude Person",
    "chore: x\n\nCo-authored-by: Claudette <c@example.com>",
    "chore: x\n\nCo-authored-by: not_claude_ <c@example.com>",
    "chore: x\n\nCo-authored-by: claude-3 <c@example.com>",
    "chore: x\n\nCo-authored-by: Human <NOREPLY@ANTHROPIC.COM>",
    "chore: x\n\nCo-authored-by: Human <h@example.com>",
    "chore: x\n\nthanks to co-authored-by: Claude",
    "chore: x\n\nClaude-Session: abc",
    "chore: x\n\n   CLAUDE-SESSION: abc",
    "chore: x\n\nnot Claude-Session: abc",
    "chore: x\n\nGenerated with Claude Code",
    "chore: x\n\nGenerated with [Claude Code](https://claude.com)",
    "chore: x\n\ngenerated with  claude code",
    "chore: x\n\nbuilt \ud83e\udd16",
    "chore: x\n\nplain body",
  ];
  return {
    slugs: SLUG_TITLES.flatMap((title) =>
      [30, 40, 1, 5].map((max) => ({ title, max, slug: slugify(title, max) })),
    ),
    branches,
    suffixes,
    subjects: subjects.map((subject) => {
      const parsed = parseCommitSubject(subject);
      return {
        subject,
        parsed: parsed.ok
          ? {
              type: parsed.type,
              scope: parsed.scope,
              breaking: parsed.breaking,
              description: parsed.description,
            }
          : null,
      };
    }),
    messages: messages.flatMap((message) =>
      [1, 2].map((parents) => ({
        message,
        parents,
        rules: checkCommitMessage(message, parents).map((r) => r.rule),
        reasons: checkCommitMessage(message, parents).map((r) => r.reason),
      })),
    ),
  } as unknown as Json;
}

// ------------------------------------------------------------------------------------------------ the export

/** Every exported file by repository-relative path, as the text the exporter writes. */
export async function exportFiles(): Promise<Map<string, string>> {
  const files = new Map<string, string>();
  const runs: Json[] = [];
  for (const scenario of scenarios()) runs.push(await runScenario(scenario));
  // The baseline is the ledger of the scenario with the fewest rows; scenarios.json records it in full.
  const rowCount = (run: Json): number =>
    Object.values((run as { tables: Tables }).tables).reduce(
      (sum, table) => sum + table.rows.length,
      0,
    );
  const smallest = runs.reduce((best, run) =>
    rowCount(run) < rowCount(best) ? run : best,
  );
  const baseline = (smallest as { tables: Tables }).tables;
  files.set(
    `${INTEGRATE_DIRECTORY}/scenarios.json`,
    packedText({
      format: 2,
      baseline: baseline as never,
      scenarios: runs.map((run) => compactSequence(run, baseline)),
    }),
  );
  files.set(
    `${INTEGRATE_DIRECTORY}/squash.json`,
    packedText({
      format: 2,
      cases: squashCases().map(({ name, info, subjects }) => {
        const message = squashMessage(info, new Map(Object.entries(subjects)));
        return stable({
          name,
          info,
          subjects,
          subject: message.subject,
          body: message.body,
        });
      }),
    }),
  );
  files.set(
    `${INTEGRATE_DIRECTORY}/conventions.json`,
    packedText({
      format: 2,
      ...(conventionCases() as { [key: string]: Json }),
    }),
  );
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

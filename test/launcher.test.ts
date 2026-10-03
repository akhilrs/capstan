import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { spawnSync } from "node:child_process";
import {
  CONFIG_FILE_NAME,
  loadCapstanConfig,
  type CapstanConfig,
  type ResolvedResearcher,
  type ResolvedRole,
  type ResolvedWorktree,
} from "../src/config/capstan-config.js";
import { ControllerCore } from "../src/controller/core.js";
import {
  Launcher,
  LauncherError,
  defaultGit,
  type GitRunner,
  runSetupCommand,
  type SetupRunner,
  type TeardownRunner,
} from "../src/launcher.js";
import { CSTAN_ALLOW_RULE } from "../src/prompts.js";
import {
  RESEARCHER_REQUIRED_DENY,
  researcherRuleProblems,
} from "../src/researcher-policy.js";
import {
  AgentPaneMismatch,
  PaneGone,
  PromptUnrecognized,
  ShellNotReady,
  buildAgentEnvironment,
} from "../src/herdr/adapter.js";
import { HerdrError } from "../src/herdr/runner.js";
import { PaneLost } from "../src/herdr/adapter.js";
import { ctx, projectInfo } from "./harness.js";
import { SHA, StubAdapter, StubGit } from "./launcher-stubs.js";

const hashOf = (name: string): string =>
  createHash("sha256").update(name).digest("hex");

function config(
  fallback = true,
  maxWorkers = 3,
  layout: Partial<CapstanConfig["layout"]> = {},
  pass: readonly string[] = [],
  hostOf: Readonly<Record<string, "codex" | "omp">> = {},
): CapstanConfig {
  const role = (
    name: string,
    kind: "PM" | "Developer" | "Supervisor",
    extra = {},
  ) => ({
    name,
    kind,
    host: hostOf[name] ?? "claude",
    model: null,
    permissionMode: "default" as const,
    allow: [] as string[],
    deny: [] as string[],
    hooks: "off" as const,
    prompt: { source: "none" as const, path: null, hash: null },
    configHash: hashOf(name),
    ...extra,
  });
  const withText = (value: ReturnType<typeof role>, text: string | null) =>
    Object.defineProperty(value, "promptText", {
      value: text,
      enumerable: false,
    });
  return {
    schemaVersion: 1,
    projectName: null,
    herdrSession: "test",
    notifications: { herdr: true, fallback },
    timers: {
      maxDeferralSeconds: 120,
      maxBusyDeferralSeconds: 120,
      pmAckTimeoutSeconds: 600,
      pmNotifyAfterSeconds: 300,
      notifyIntervalSeconds: 600,
      stallAfterSeconds: 900,
      workerAckTimeoutSeconds: 600,
      pmWakeAfterSeconds: 0,
      pmWakeIntervalSeconds: 120,
      findingCheckSeconds: 1800,
    },
    limits: { maxWorkers },
    layout: {
      spawn: "tab",
      pmWidthPercent: 60,
      minPaneColumns: 60,
      minPaneRows: 12,
      ...layout,
    },
    env: { pass },
    hosts: [
      {
        name: "claude",
        kind: "claude",
        command: "claude",
        shellCommandTimeoutSeconds: 120,
        waitTimeoutSeconds: 45,
      },
      {
        name: "codex",
        kind: "codex",
        command: "codex",
        shellCommandTimeoutSeconds: 120,
        waitTimeoutSeconds: 45,
      },
      {
        name: "omp",
        kind: "omp",
        command: "omp",
        shellCommandTimeoutSeconds: 120,
        waitTimeoutSeconds: 45,
      },
    ],
    roles: [
      withText(role("pm", "PM"), "Keep the plan small."),
      withText(role("developer", "Developer"), null),
      withText(role("developer2", "Developer"), null),
      withText(role("supervisor", "Supervisor"), null),
    ],
  } as unknown as CapstanConfig;
}

function withArchitect(
  base: CapstanConfig,
  architect: { readonly counts: boolean } | undefined,
): CapstanConfig {
  if (architect === undefined) return base;
  const developer = base.roles.find((r) => r.name === "developer")!;
  const role = Object.defineProperty(
    { ...developer, name: "architect", configHash: hashOf("architect") },
    "promptText",
    { value: null, enumerable: false },
  );
  return {
    ...base,
    roles: [...base.roles, role],
    architect: {
      enabled: true,
      role: "architect",
      planReview: "high_risk",
      reviewerRole: null,
      maxPackages: 8,
      countTowardWorkerLimit: architect.counts,
      highRiskTriggers: ["schema or migrations"],
    },
  } as CapstanConfig;
}

function withResearcher(
  base: CapstanConfig,
  researcher: { readonly enabled: boolean } | undefined,
): CapstanConfig {
  if (researcher === undefined) return base;
  const developer = base.roles.find((r) => r.name === "developer")!;
  const role = Object.defineProperty(
    {
      ...developer,
      name: "researcher",
      configHash: hashOf("researcher"),
      allow: ["WebSearch", "Bash(jq *)", "mcp__playwright__browser_navigate"],
      mcp: [
        {
          name: "playwright",
          command: "npx",
          args: ["-y", "@playwright/mcp@latest", "--headless"],
        },
      ],
    },
    "promptText",
    { value: null, enumerable: false },
  );
  return {
    ...base,
    roles: [...base.roles, role],
    researcher: {
      configured: true,
      enabled: researcher.enabled,
      role: "researcher",
      outputDir: "docs/research",
      userAgent: "capstan-researcher/1.0 (test)",
    },
  } as unknown as CapstanConfig;
}

function withOperator(
  base: CapstanConfig,
  operator:
    | {
        readonly counts: boolean;
        readonly enabled: boolean;
        /** False: the role exists but the configuration has no [operator] table. */
        readonly table?: boolean;
      }
    | undefined,
): CapstanConfig {
  if (operator === undefined) return base;
  const developer = base.roles.find((r) => r.name === "developer")!;
  const role = Object.defineProperty(
    { ...developer, name: "operator", configHash: hashOf("operator") },
    "promptText",
    { value: null, enumerable: false },
  );
  return {
    ...base,
    roles: [...base.roles, role],
    ...(operator.table === false
      ? {}
      : {
          operator: {
            configured: true,
            enabled: operator.enabled,
            role: "operator",
            autoApprove: ["ls -l"],
            autoApprovePrefix: [],
            countTowardWorkerLimit: operator.counts,
          },
        }),
  } as unknown as CapstanConfig;
}

interface World {
  core: ControllerCore;
  owner: string;
  adapter: StubAdapter;
  git: StubGit;
  launcher: Launcher;
  root: string;
  events: Array<{ event: string; details: Record<string, unknown> }>;
  /** The delays the launcher asked for between start attempts. */
  sleeps: number[];
  reopen(syncRoles?: () => void): Launcher;
  cleanup(): void;
}

async function world(
  fallback = true,
  synced = true,
  maxWorkers = 3,
  layout: Partial<CapstanConfig["layout"]> = {},
  environment: {
    readonly pass?: readonly string[];
    readonly base?: Readonly<Record<string, string>>;
    readonly hostOf?: Readonly<Record<string, "codex" | "omp">>;
    /** Enables the Architect role in the configuration; `counts` is count_toward_worker_limit. */
    readonly architect?: { readonly counts: boolean };
    /** Adds the Operator role to the configuration, enabled or not. */
    readonly operator?: {
      readonly counts: boolean;
      readonly enabled: boolean;
      readonly table?: boolean;
    };
    /** Adds the Researcher role and a [researcher] table, enabled or not. */
    readonly researcher?: { readonly enabled: boolean };
    readonly worktree?: ResolvedWorktree;
    readonly runSetup?: SetupRunner;
    readonly runTeardown?: TeardownRunner;
    readonly git?: GitRunner;
    /** Sets `[prompt_relay] enabled`. */
    readonly promptRelay?: boolean;
  } = {},
): Promise<World> {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-launcher-"));
  const stateDirectory = path.join(root, ".capstan", "state");
  const info = projectInfo();
  const core = await ControllerCore.open({ stateDirectory, project: info });
  const owner = info.ownerCredential;
  if (synced)
    core.syncRoleDefinitions(
      ctx(core, owner),
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
        const [name, kind] = entry.split(":") as [
          string,
          "PM" | "Developer" | "Supervisor",
        ];
        return {
          name,
          kind,
          host: environment.hostOf?.[name] ?? "claude",
          configHash: hashOf(name),
        };
      }),
    );
  const adapter = new StubAdapter();
  const git = new StubGit();
  const events: World["events"] = [];
  const sleeps: number[] = [];
  const make = (syncRoles?: () => void): Launcher =>
    new Launcher({
      core,
      adapter,
      config: {
        ...withResearcher(
          withOperator(
            withArchitect(
              config(
                fallback,
                maxWorkers,
                layout,
                environment.pass,
                environment.hostOf,
              ),
              environment.architect,
            ),
            environment.operator,
          ),
          environment.researcher,
        ),
        ...(environment.worktree === undefined
          ? {}
          : { worktree: environment.worktree }),
        ...(environment.promptRelay === undefined
          ? {}
          : {
              promptRelay: {
                present: true,
                enabled: environment.promptRelay,
                captureTtlSeconds: 300,
              },
            }),
      },
      ...(environment.runSetup === undefined
        ? {}
        : { runSetup: environment.runSetup }),
      ...(environment.runTeardown === undefined
        ? {}
        : { runTeardown: environment.runTeardown }),
      projectRoot: root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(stateDirectory, "control.sock"),
      credential: owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: {
        PATH: "/usr/bin:/bin",
        HOME: "/home/x",
        LANG: "C",
        SECRET: "no",
        ...environment.base,
      },
      git: environment.git ?? git,
      sleep: async (milliseconds) => {
        sleeps.push(milliseconds);
      },
      log: (event, details) => events.push({ event, details }),
      ...(syncRoles === undefined ? {} : { syncRoles }),
    });
  return {
    core,
    owner,
    adapter,
    git,
    launcher: make(),
    root,
    events,
    sleeps,
    reopen: make,
    cleanup: () => {
      core.close();
      rmSync(root, { recursive: true, force: true });
      rmSync(adapter.dir, { recursive: true, force: true });
    },
  };
}

const eventNames = (w: World): string[] => w.events.map((e) => e.event);

test("launch creates the PM agent, starts it with its token, prompt and a cstan wrapper, records the pane and opens the watch pane", async () => {
  const w = await world();
  try {
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "started");
    assert.equal(result.agentId, "pm-1");
    assert.equal(result.hub, "opened");
    const agent = w.core.listAgents().find((a) => a.agentId === "pm-1")!;
    assert.equal(agent.kind, "PM");
    assert.equal(agent.state, "active");
    const start = w.adapter.starts[0]!;
    assert.equal(start.name, "pm-1");
    const environment = start.environment!;
    assert.match(environment.CAPSTAN_TOKEN!, /\S{20,}/);
    assert.equal(
      environment.CAPSTAN_SOCKET,
      path.join(w.root, ".capstan", "state", "control.sock"),
    );
    assert.equal(environment.PATH, `${w.root}/.capstan/bin:/usr/bin:/bin`);
    assert.equal(environment.SECRET, undefined, "only the allowlist passes");
    assert.ok(start.args.includes(CSTAN_ALLOW_RULE));
    const promptFile =
      start.args[start.args.indexOf("--append-system-prompt-file") + 1]!;
    const prompt = readFileSync(promptFile, "utf8");
    assert.match(prompt, /cstan inbox/);
    assert.match(prompt, /45 seconds/);
    assert.match(prompt, /Keep the plan small\./);
    assert.ok(
      !prompt.includes(environment.CAPSTAN_TOKEN!),
      "the token is never in the prompt",
    );
    const rows = w.core.agentPanes(w.owner);
    assert.equal(rows.length, 1);
    assert.equal(rows[0]!.agentId, "pm-1");
    assert.equal(rows[0]!.paneId, result.paneId);
    const fallback = w.core.fallbackPane(w.owner)!;
    assert.ok(
      w.adapter.calls.some(
        (c) =>
          c.startsWith(`run:${fallback.paneId}:`) &&
          c.includes("status --watch"),
      ),
    );
    assert.equal(
      w.adapter.lastShellEnvironment!.CAPSTAN_TOKEN,
      undefined,
      "the watch pane holds no token",
    );
    assert.equal(w.adapter.lastShellEnvironment!.CAPSTAN_SOCKET, undefined);
  } finally {
    w.cleanup();
  }
});

test("the cstan wrapper is a private script that runs the CLI with the recorded node, and is replaced atomically", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const wrapper = path.join(w.root, ".capstan", "bin", "cstan");
    assert.equal(statSync(wrapper).mode & 0o777, 0o700);
    assert.equal(statSync(path.dirname(wrapper)).mode & 0o777, 0o700);
    const text = readFileSync(wrapper, "utf8");
    assert.equal(
      text,
      `#!/bin/sh\nexec '/usr/bin/node' '/opt/capstan/cli.js' "$@"\n`,
    );

    const fake = path.join(w.root, "fake-node");
    const cli = path.join(w.root, "cli.js");
    writeFileSync(fake, '#!/bin/sh\necho "ran $1 $2"\n', { mode: 0o755 });
    chmodSync(fake, 0o755);
    writeFileSync(cli, "");
    writeFileSync(wrapper, `#!/bin/sh\nexec '${fake}' '${cli}' "$@"\n`, {
      mode: 0o700,
    });
    const run = spawnSync(wrapper, ["status"], { encoding: "utf8" });
    assert.equal(run.status, 0);
    assert.match(run.stdout, /ran .*cli.js status/);
  } finally {
    w.cleanup();
  }
});

test("a second launch reports the running PM and opens nothing new; a blocked PM is reported; a PM without a pane needs a restart", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const calls = w.adapter.calls.length;
    const again = await w.launcher.launchPm();
    assert.equal(again.state, "running");
    assert.equal(again.hub, "present");
    assert.equal(
      w.adapter.calls.length,
      calls,
      "no workspace, start or pane was created",
    );
    w.adapter.observation = "blocked";
    assert.equal((await w.launcher.launchPm()).state, "blocked");

    w.adapter.agentPanes.clear();
    const lost = await w.launcher.launchPm();
    assert.equal(lost.state, "needs_restart");
    assert.match(lost.hint!, /pm restart/);
  } finally {
    w.cleanup();
  }
});

test("a PM that is blocked at startup is reported with the hint and keeps its pane row; the hub opens even with the fallback channel off", async () => {
  const w = await world(false);
  try {
    w.adapter.startStatus = "blocked_at_startup";
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "blocked");
    assert.match(result.hint!, /trust dialog/);
    assert.equal(
      result.hub,
      "opened",
      "the hub opens whatever the notification channels say",
    );
    assert.equal(w.core.agentPanes(w.owner).length, 1);
    assert.ok(
      !w.adapter.calls.some((c) => c.startsWith("dialog:")),
      "the PM's dialog is never answered",
    );
  } finally {
    w.cleanup();
  }
});

test("a failed first start ends the new agent, closes its pane and leaves nothing behind, and a later launch starts fresh", async () => {
  const w = await world();
  try {
    w.adapter.startError = new PromptUnrecognized("the shell is not ready");
    const failed = await w.launcher.launchPm();
    assert.equal(failed.state, "failed");
    assert.match(failed.reason!, /shell is not ready/);
    assert.ok(w.adapter.calls.some((c) => c.startsWith("close:")));
    assert.equal(w.core.agentPanes(w.owner).length, 0);
    assert.equal(
      w.core.listAgents().filter((a) => a.state === "active").length,
      0,
    );

    w.adapter.startError = undefined;
    const fresh = await w.launcher.launchPm();
    assert.equal(fresh.state, "started");
    assert.equal(
      fresh.agentId,
      "pm-2",
      "a new incarnation gets a new id on the same seat",
    );
  } finally {
    w.cleanup();
  }
});

test("a crash between registering the PM and recording its pane leaves an active PM with no row: launch says needs_restart", async () => {
  const w = await world();
  try {
    const seat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "pm-seat",
      name: "pm",
      role: "PM",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "pm-1",
      role: "PM",
      seatId: seat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "pm-1",
      roleName: "pm",
      seatId: seat.seatId,
      actorId: actor.actorId,
    });
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "needs_restart");
  } finally {
    w.cleanup();
  }
});

test("two active PMs make launch refuse with pm_exists", async () => {
  const w = await world();
  try {
    for (const [name, seatName] of [
      ["pm", "pm"],
      ["pm2", "pm2"],
    ] as const) {
      const seat = w.core.createSeat(ctx(w.core, w.owner), {
        seatId: `${seatName}-seat`,
        name,
        role: "PM",
      });
      const actor = w.core.createActor(ctx(w.core, w.owner), {
        displayName: name,
        role: "PM",
        seatId: seat.seatId,
      });
      w.core.registerAgent(ctx(w.core, w.owner), {
        agentId: `${name}-1`,
        roleName: name,
        seatId: seat.seatId,
        actorId: actor.actorId,
      });
    }
    await assert.rejects(
      w.launcher.launchPm(),
      (e: unknown) => e instanceof LauncherError && e.code === "pm_exists",
    );
    await assert.rejects(
      w.launcher.restartPm(),
      (e: unknown) => e instanceof LauncherError && e.code === "pm_ambiguous",
    );
  } finally {
    w.cleanup();
  }
});

async function launched(w: World): Promise<void> {
  assert.equal((await w.launcher.launchPm()).state, "started");
}

test("spawn starts one worker in its own worktree with its token, prompt, worker profile and a base sha, and records the row", async () => {
  const w = await world();
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.equal(result.agentId, "developer-1");
    assert.equal(result.branch, "capstan/developer-1-g1");
    assert.ok(
      w.adapter.calls.includes(`worktree:capstan/developer-1-g1:${SHA}`),
    );
    assert.deepEqual(
      w.adapter.worktreeParents,
      [w.core.fallbackPane(w.owner)!.workspaceId],
      "worktrees hang under the hub workspace, never under the PM's",
    );
    const start = w.adapter.starts.find((s) => s.name === "developer-1")!;
    assert.ok(start.args.includes(CSTAN_ALLOW_RULE));
    assert.ok(start.args.includes("--settings"), "hooks are off for a worker");
    assert.match(start.environment!.CAPSTAN_TOKEN!, /\S{20,}/);
    assert.equal(
      start.environment!.PATH!.startsWith(`${w.root}/.capstan/bin:`),
      true,
    );
    const prompt = readFileSync(
      start.args[start.args.indexOf("--append-system-prompt-file") + 1]!,
      "utf8",
    );
    assert.match(prompt, /cstan ack <message-id>/);
    assert.match(prompt, /cstan send @pm/);
    const row = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.deepEqual(
      [row.worktreePath, row.branch, row.baseSha, row.paneId === result.paneId],
      ["/tmp/work/developer-1", "capstan/developer-1-g1", SHA, true],
    );
  } finally {
    w.cleanup();
  }
});

test("launch on a project whose roles were never synced says so and leaves no seat, actor or agent behind", async () => {
  const w = await world(true, false);
  try {
    const seats = w.core.statusSnapshot().roles.length;
    await assert.rejects(
      w.launcher.launchPm(),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "role_not_synced" &&
        e.message.includes("pm") &&
        e.message.includes("cstan start"),
    );
    assert.equal(w.core.listAgents().length, 0);
    assert.equal(w.core.statusSnapshot().roles.length, seats);
    assert.equal(w.adapter.calls.length, 0);
  } finally {
    w.cleanup();
  }
});

test("spawn of a role that is not synced says so before it touches Herdr", async () => {
  const w = await world();
  try {
    await launched(w);
    w.core.syncRoleDefinitions(
      ctx(w.core, w.owner),
      ["pm:PM", "pm2:PM", "developer2:Developer"].map((entry) => {
        const [name, kind] = entry.split(":") as [string, "PM" | "Developer"];
        return {
          name,
          kind,
          host: "claude",
          configHash: hashOf(name),
        };
      }),
    );
    const calls = w.adapter.calls.length;
    const agents = w.core.listAgents().length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "role_not_synced" &&
        e.message.includes("developer"),
    );
    assert.equal(w.adapter.calls.length, calls);
    assert.equal(w.core.listAgents().length, agents);
  } finally {
    w.cleanup();
  }
});

test("a role whose definition changed is synced again on demand, and a sync that fails is named in the refusal", async () => {
  const w = await world();
  try {
    const stale = (hash: string) =>
      w.core.syncRoleDefinitions(
        ctx(w.core, w.owner),
        ["pm", "pm2", "developer", "developer2"].map((name) => ({
          name,
          kind: name.startsWith("pm")
            ? ("PM" as const)
            : ("Developer" as const),
          host: "claude",
          configHash: name === "pm" ? hash : hashOf(name),
        })),
      );
    stale("f".repeat(64));
    await assert.rejects(
      w.launcher.launchPm(),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "role_not_synced",
    );
    assert.equal(w.core.listAgents().length, 0);
    const failing = w.reopen(() => {
      throw new Error(
        "the ledger refused\nthe \u001b[31mchange\u001b[0m ".concat(
          "x".repeat(400),
        ),
      );
    });
    await assert.rejects(
      failing.launchPm(),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "role_not_synced" &&
        e.message.includes("the ledger refused the") &&
        e.message.includes("role_sync_failed") &&
        !/[\p{Cc}]/u.test(e.message) &&
        e.message.length < 450,
    );
    const healing = w.reopen(() => stale(hashOf("pm")));
    assert.equal((await healing.launchPm()).state, "started");
  } finally {
    w.cleanup();
  }
});

test("spawn refuses an unknown role, a PM role and a missing PM, without side effects", async () => {
  const w = await world();
  try {
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "pm_not_launched",
    );
    await launched(w);
    await w.launcher.spawn("developer");
    const calls = w.adapter.calls.length;
    const agents = w.core.listAgents().length;
    await assert.rejects(
      w.launcher.spawn("nobody"),
      (e: unknown) => e instanceof LauncherError && e.code === "unknown_role",
    );
    await assert.rejects(
      w.launcher.spawn("pm"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "kind_not_spawnable",
    );
    assert.equal(w.adapter.calls.length, calls);
    assert.equal(w.core.listAgents().length, agents);
    const other = await w.launcher.spawn("developer2");
    assert.equal(other.agentId, "developer2-1", "another role is independent");
  } finally {
    w.cleanup();
  }
});

test("several workers of one role get their own seats, ids, branches and worktrees up to the limit, and the next spawn names who is active", async () => {
  const w = await world();
  try {
    await launched(w);
    const one = await w.launcher.spawn("developer");
    const two = await w.launcher.spawn("developer");
    const three = await w.launcher.spawn("developer2");
    assert.deepEqual(
      [one.agentId, two.agentId, three.agentId],
      ["developer-1", "developer-2", "developer2-1"],
    );
    assert.notEqual(one.branch, two.branch);
    assert.notEqual(one.worktreePath, two.worktreePath);
    const seats = w.core
      .listAgents()
      .filter((a) => a.roleName === "developer")
      .map((a) => a.seatId);
    assert.deepEqual(seats, ["developer-seat", "developer-seat-2"]);
    const calls = w.adapter.calls.length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worker_limit" &&
        e.message.includes("3 of 3") &&
        e.message.includes("developer-1") &&
        e.message.includes("developer2-1"),
    );
    assert.equal(w.adapter.calls.length, calls, "nothing was started");
  } finally {
    w.cleanup();
  }
});

test("release ends a worker, closes its pane, removes its worktree and an unchanged branch, and frees its slot without reusing the id", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const released = await w.launcher.release(first.agentId);
    assert.equal(released.state, "released");
    assert.equal(released.agentId, "developer-1");
    assert.equal(released.branch, first.branch);
    assert.deepEqual(
      [released.paneClosed, released.worktreeRemoved, released.branchKept],
      [true, true, false],
    );
    assert.equal(w.core.agentRecord("developer-1")!.state, "ended");
    assert.ok(w.adapter.calls.includes(`close:${first.paneId}`));
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    assert.equal(w.git.deleted[0]![0], first.branch);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
      "the pane row is gone",
    );
    const again = await w.launcher.spawn("developer");
    assert.equal(again.agentId, "developer-2", "an ended id is not reused");
  } finally {
    w.cleanup();
  }
});

test("release keeps a branch that holds commits and reports a worktree git refused to remove", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.git.deleteOk = false;
    const kept = await w.launcher.release(first.agentId);
    assert.deepEqual(
      [kept.worktreeRemoved, kept.branchKept],
      [true, true],
      "the user merges a branch with commits",
    );
    const second = await w.launcher.spawn("developer");
    w.git.removeOk = false;
    const dirty = await w.launcher.release(second.agentId);
    assert.deepEqual(
      [dirty.paneClosed, dirty.worktreeRemoved, dirty.branchKept],
      [true, false, true],
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === second.agentId),
      true,
      "the row stays so the next start retries the removal",
    );
  } finally {
    w.cleanup();
  }
});

test("release keeps the pane row, worktree and branch when the pane will not close, and the next operation finishes the job", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const stuck = await w.launcher.release(first.agentId);
    assert.deepEqual(
      [stuck.paneClosed, stuck.worktreeRemoved, stuck.branchKept],
      [false, false, true],
    );
    assert.deepEqual(
      w.git.removed,
      [],
      "nothing was removed under an open pane",
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === first.agentId),
      true,
    );
    w.adapter.closeError = undefined;
    await w.launcher.spawn("developer");
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === first.agentId),
      false,
      "the retry cleared the row",
    );
  } finally {
    w.cleanup();
  }
});

test("an extra seat gets a dotted display name, so a role literally named like it still gets its own seat", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer");
    assert.throws(
      () =>
        w.core.createSeat(ctx(w.core, w.owner), {
          seatId: "other-seat",
          name: "developer.2",
          role: "Developer",
        }),
      "the extra seat already holds the display name developer.2",
    );
    const roleSeat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-2-seat",
      name: "developer-2",
      role: "Developer",
    });
    assert.equal(roleSeat.seatId, "developer-2-seat");
  } finally {
    w.cleanup();
  }
});

const PANE = { spawn: "pane" as const };

test("pane mode splits the worker into the PM's tab, records the new pane id and workspace, and stacks the next worker", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!;
    const first = await w.launcher.spawn("developer");
    assert.equal(first.placement, "pane");
    assert.equal(first.placementNote, undefined);
    assert.ok(
      w.adapter.calls.some((c) => c.endsWith(`:${pmPane.paneId}:right`)),
      "a wide PM pane is split to the right",
    );
    assert.notEqual(first.paneId, "w3:p1");
    const row = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.equal(row.paneId, first.paneId, "the ledger holds the new pane id");
    assert.equal(row.workspaceId, w.adapter.pmWorkspace);
    assert.equal(w.adapter.starts.at(-1)!.paneId, first.paneId);
    const second = await w.launcher.spawn("developer2");
    assert.equal(second.placement, "pane");
    assert.ok(
      w.adapter.calls.some((c) => c.endsWith(`:${first.paneId}:down`)),
      "the next worker stacks below the first worker, in the column on the PM's right",
    );
    assert.ok(
      !w.adapter.calls.some((c) => c.endsWith(`:${pmPane.paneId}:down`)),
      "the PM pane is never split down",
    );
    const layout = w.adapter.tabPanes.find((p) => p.paneId === pmPane.paneId)!;
    assert.equal(layout.height, 50, "the PM pane keeps the full height");
    assert.equal(layout.width, 120, "the PM pane keeps 60% of the width");
  } finally {
    w.cleanup();
  }
});

test("the PM keeps pm_width_percent of its tab when the first worker is placed", async () => {
  const w = await world(true, true, 3, { spawn: "pane", pmWidthPercent: 70 });
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    assert.deepEqual(w.adapter.keeps, [0.7]);
    await w.launcher.spawn("developer2");
    assert.deepEqual(w.adapter.keeps, [0.7, 0.5]);
  } finally {
    w.cleanup();
  }
});

test("a worker stays a tab, with the reason, when nothing fits, the layout fails, the tab is zoomed or the move fails", async () => {
  const w = await world(true, true, 3, {
    spawn: "pane",
    minPaneColumns: 150,
    minPaneRows: 30,
  });
  try {
    await launched(w);
    const tooSmall = await w.launcher.spawn("developer");
    assert.equal(tooSmall.placement, "tab");
    assert.match(
      tooSmall.placementNote!,
      /no pane has room.*150 columns by 30 rows/,
    );
    assert.ok(!w.adapter.calls.some((c) => c.startsWith("place:")));
    const row = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.equal(row.paneId, tooSmall.paneId);
  } finally {
    w.cleanup();
  }
  const x = await world(true, true, 3, PANE);
  try {
    await launched(x);
    x.adapter.layoutError = new Error(
      "layout\nfailed \u001b[31mhard\u001b[0m " + "x".repeat(400),
    );
    const a = await x.launcher.spawn("developer");
    assert.equal(a.placement, "tab");
    assert.ok(
      a.placementNote!.startsWith(
        "the pane could not be placed (layout failed",
      ),
    );
    assert.ok(!/[\p{Cc}]/u.test(a.placementNote!));
    assert.ok(a.placementNote!.length < 260);
    x.adapter.layoutError = undefined;
    x.adapter.zoomed = true;
    const b = await x.launcher.spawn("developer");
    assert.equal(b.placement, "tab");
    assert.match(b.placementNote!, /zoomed/);
    x.adapter.zoomed = false;
    x.adapter.placeError = new Error("move refused");
    const c = await x.launcher.spawn("developer");
    assert.equal(c.placement, "tab");
    assert.match(c.placementNote!, /move refused/);
    const rowC = x.core
      .agentPanes(x.owner)
      .find((r) => r.agentId === c.agentId)!;
    assert.equal(rowC.paneId, c.paneId);
    assert.deepEqual(
      [a, b, c].map((r) => x.core.agentRecord(r.agentId)!.state),
      ["active", "active", "active"],
      "layout never fails a spawn",
    );
  } finally {
    x.cleanup();
  }
});

test("tab mode never asks for a layout", async () => {
  const w = await world();
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.placement, "tab");
    assert.ok(
      !w.adapter.calls.some(
        (c) => c.startsWith("layout:") || c.startsWith("place:"),
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("release closes a placed worker's new pane and removes its worktree, and a failed start after the move closes the new pane", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const released = await w.launcher.release(first.agentId);
    assert.equal(released.paneClosed, true);
    assert.ok(w.adapter.calls.includes(`close:${first.paneId}`));
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    w.adapter.startError = new Error("start failed");
    await assert.rejects(w.launcher.spawn("developer"));
    const placed = w.adapter.calls
      .filter((c) => c.startsWith("place:"))
      .at(-1)!;
    assert.ok(placed.startsWith("place:"));
    assert.ok(
      w.adapter.calls.some((c) => /^close:w\d+:p1\d$/.test(c)),
      "cleanup closed the pane at its new id",
    );
  } finally {
    w.cleanup();
  }
});

test("a pane lost in the move fails the spawn, and cleanup closes the one unregistered pane at the worktree path", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    w.adapter.placeError = new PaneLost("gone");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set("/tmp/work/developer-1", [
      { paneId: "w9:p42", workspaceId: w.adapter.pmWorkspace! },
    ]);
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.message === "step place: gone",
    );
    assert.ok(
      w.adapter.calls.includes("close:w9:p42"),
      "the moved pane was found by its path and closed",
    );
    assert.equal(w.core.agentRecord("developer-1")!.state, "ended");
  } finally {
    w.cleanup();
  }
});

test("cleanup leaves panes at the worktree path alone when more than one matches or the recorded pane still exists", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    w.adapter.placeError = new PaneLost("gone");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set("/tmp/work/developer-1", [
      { paneId: "w9:p42", workspaceId: w.adapter.pmWorkspace! },
      { paneId: "w9:p43", workspaceId: w.adapter.pmWorkspace! },
    ]);
    await assert.rejects(w.launcher.spawn("developer"));
    assert.ok(!w.adapter.calls.includes("close:w9:p42"));
    assert.ok(!w.adapter.calls.includes("close:w9:p43"));
  } finally {
    w.cleanup();
  }
});

test("after a crash between the move and the ledger write, adoption closes the moved pane found at the worktree path in pane mode and leaves panes alone in tab mode", async () => {
  for (const [layout, closed] of [
    [{ spawn: "pane" as const }, true],
    [{ spawn: "tab" as const }, false],
  ] as const) {
    const w = await world(true, true, 3, layout);
    try {
      await launched(w);
      const first = await w.launcher.spawn("developer");
      w.core.recordAgentPane(ctx(w.core, w.owner), {
        agentId: first.agentId,
        workspaceId: "w7",
        paneId: "w7:p1",
        worktreePath: first.worktreePath,
        branch: first.branch,
        baseSha: SHA,
      });
      w.adapter.adoptErrors.set("w7:p1", new PaneGone("gone"));
      w.adapter.closeMissingThrows = true;
      w.adapter.strays.set(first.worktreePath, [
        { paneId: "w1:p99", workspaceId: w.adapter.pmWorkspace! },
      ]);
      await w.launcher.adoptAll();
      assert.equal(
        w.adapter.calls.includes("close:w1:p99"),
        closed,
        JSON.stringify(layout),
      );
      assert.equal(w.core.agentRecord(first.agentId)!.state, "ended");
    } finally {
      w.cleanup();
    }
  }
});

test("a pane at the worktree path in another workspace is the operator's and is never closed", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    w.adapter.placeError = new PaneLost("gone");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set("/tmp/work/developer-1", [
      { paneId: "w8:p1", workspaceId: "w8" },
    ]);
    await assert.rejects(w.launcher.spawn("developer"));
    assert.ok(!w.adapter.calls.includes("close:w8:p1"));
  } finally {
    w.cleanup();
  }
});

test("a placement note keeps no escape sequence of any kind and stays short even for combining marks", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const hostile = [
      "osc \u001b]0;evil title\u0007 end",
      "dcs \u001bPpayload\u001b\\ end",
      "c1 \u009b31mred\u009b0m end",
      "surrogate \ud800 end",
      "marks e" + "\u0301".repeat(5000),
      "unterminated \u001b]0;evil title",
      "8bit \u009d0;evil osc\u009c end",
      "8bit dcs \u0090payload\u009c end",
      "colon \u001b[38:2:255:0:0m red",
      "two byte \u001bc reset",
      "e".repeat(10) + "\u0301".repeat(500),
    ];
    for (const message of hostile) {
      w.adapter.layoutError = new Error(message);
      const result = await w.launcher.spawn("developer");
      assert.equal(result.placement, "tab");
      const note = result.placementNote!;
      assert.ok(
        !/evil title|payload|31m|0m|\u001b|\u009b|\ud800/.test(note),
        note,
      );
      assert.ok(note.length < 300, `${note.length}`);
      await w.launcher.release(result.agentId);
    }
  } finally {
    w.cleanup();
  }
});

test("a gone pane is not searched for when no move was interrupted: a pane recorded in the PM's workspace, or a normal release", async () => {
  const w = await world(true, true, 3, PANE);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeMissingThrows = true;
    w.adapter.strays.set(first.worktreePath, [
      { paneId: "w1:p77", workspaceId: w.adapter.pmWorkspace! },
    ]);
    // The worker's pane died after it was recorded in the PM's workspace; a release must not look for strays.
    w.adapter.tabPanes = w.adapter.tabPanes.filter(
      (p) => p.paneId !== first.paneId,
    );
    w.adapter.entries.delete(first.paneId);
    await w.launcher.release(first.agentId);
    assert.ok(!w.adapter.calls.some((c) => c.startsWith("panes-at:")));
    assert.ok(!w.adapter.calls.includes("close:w1:p77"));
  } finally {
    w.cleanup();
  }
  const x = await world(true, true, 3, PANE);
  try {
    await launched(x);
    const second = await x.launcher.spawn("developer");
    x.adapter.adoptErrors.set(second.paneId, new PaneGone("gone"));
    x.adapter.closeMissingThrows = true;
    x.adapter.strays.set(second.worktreePath, [
      { paneId: "w1:p77", workspaceId: x.adapter.pmWorkspace! },
    ]);
    await x.launcher.adoptAll();
    assert.ok(
      !x.adapter.calls.some((c) => c.startsWith("panes-at:")),
      "the row names the PM's workspace, so the pane died; nothing was mid-move",
    );
  } finally {
    x.cleanup();
  }
});

test("the branch is named for the generation, git must accept the name before anything is created, and an older row keeps the name it holds", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    assert.equal(first.branch, "capstan/developer-1-g1");
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === "developer-1")!
        .branch,
      "capstan/developer-1-g1",
    );
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "developer-1",
      workspaceId: "w3",
      paneId: first.paneId,
      worktreePath: first.worktreePath,
      branch: "capstan/developer-1",
      baseSha: SHA,
    });
    await w.launcher.release("developer-1");
    assert.equal(
      w.git.deleted.at(-1)![0],
      "capstan/developer-1",
      "cleanup uses the recorded name, whichever it is",
    );
    w.git.branchNamesValid = false;
    const calls = w.adapter.calls.length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) => e instanceof LauncherError && e.code === "invalid_branch",
    );
    assert.ok(
      !w.adapter.calls.slice(calls).some((c) => c.startsWith("worktree:")),
      "no worktree was created for a name git refuses",
    );
    assert.equal(w.core.agentRecord("developer-2")!.state, "ended");
  } finally {
    w.cleanup();
  }
});

test("spawn can start a worker's worktree and branch at a given commit, and refuses a base that is not a full id", async () => {
  const w = await world();
  try {
    await launched(w);
    const at = "c".repeat(40);
    const first = await w.launcher.spawn("developer", { baseSha: at });
    assert.ok(w.adapter.calls.includes(`worktree:${first.branch}:${at}`));
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === first.agentId)!
        .baseSha,
      at,
    );
    for (const bad of ["abc", "C".repeat(40), `${at} `, ""])
      await assert.rejects(
        w.launcher.spawn("developer", { baseSha: bad }),
        (e: unknown) => e instanceof LauncherError && e.code === "invalid_base",
        JSON.stringify(bad),
      );
    await w.launcher.release(first.agentId);
    assert.deepEqual(
      w.git.deleted.at(-1),
      [first.branch, at],
      "the review branch is deleted while it still points at the reviewed commit",
    );
  } finally {
    w.cleanup();
  }
});

test("a freed seat is reused before a new one is created", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer");
    await w.launcher.release("developer-1");
    const third = await w.launcher.spawn("developer");
    assert.equal(third.agentId, "developer-3");
    assert.equal(w.core.agentRecord("developer-3")!.seatId, "developer-seat");
  } finally {
    w.cleanup();
  }
});

test("release refuses an unknown agent, the PM and an agent that was already released, and reports a cleanup that cannot end the agent", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const code = (error: unknown) =>
      error instanceof LauncherError ? error.code : String(error);
    await assert.rejects(
      w.launcher.release("nobody-9"),
      (e: unknown) => code(e) === "unknown_agent",
    );
    await assert.rejects(
      w.launcher.release("pm-1"),
      (e: unknown) => code(e) === "kind_not_releasable",
    );
    const endAgent = w.core.endAgent.bind(w.core);
    (w.core as unknown as { endAgent: () => never }).endAgent = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(
      w.launcher.release(first.agentId),
      (e: unknown) =>
        code(e) === "release_blocked" &&
        /still holds authority/.test((e as Error).message),
    );
    assert.equal(w.core.agentRecord(first.agentId)!.state, "active");
    (w.core as unknown as { endAgent: typeof endAgent }).endAgent = endAgent;
    await w.launcher.release(first.agentId);
    assert.deepEqual(
      w.launcher.status().cleanupFailed,
      [],
      "a later successful release clears the failure",
    );
    await assert.rejects(
      w.launcher.release(first.agentId),
      (e: unknown) => code(e) === "agent_not_active",
    );
  } finally {
    w.cleanup();
  }
});

test("a worker blocked at startup has its trust dialog answered once with every key logged; a refused answer is reported", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startStatus = "blocked_at_startup";
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.equal(
      w.adapter.calls.filter((c) => c.startsWith("dialog:")).length,
      1,
    );
    assert.deepEqual(
      w.events
        .filter((e) => e.event === "trust_dialog_key")
        .map((e) => e.details.key),
      ["down", "enter"],
    );
    w.adapter.dialogHandled = false;
    const refused = await w.launcher.spawn("developer2");
    assert.equal(refused.state, "blocked");
    assert.match(refused.hint!, /path_mismatch/);
  } finally {
    w.cleanup();
  }
});

test("a failed spawn ends the agent, closes the pane, removes the worktree, deletes the branch at the base sha and clears the row; a retry works", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new HerdrError("agent_start_failed", "no binary");
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "agent_start_failed",
    );
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer-1")!.state,
      "ended",
    );
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
    const order = w.adapter.calls.filter(
      (c) => c.startsWith("close:") || c.startsWith("worktree:"),
    );
    assert.ok(order.length >= 2);

    w.adapter.startError = undefined;
    const retry = await w.launcher.spawn("developer");
    assert.equal(retry.agentId, "developer-2");
    assert.equal(retry.branch, "capstan/developer-2-g1");
  } finally {
    w.cleanup();
  }
});

test("cleanup never forces: a worktree git refuses to remove keeps its branch and is reported; a kept branch is logged", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    w.git.removeOk = false;
    await assert.rejects(w.launcher.spawn("developer"));
    assert.deepEqual(
      w.git.deleted,
      [],
      "the branch is not touched while the worktree stays",
    );
    assert.deepEqual(w.launcher.status().cleanupFailed, [
      {
        agentId: "developer-1",
        reason: "git could not remove the worktree",
        worktreePath: "/tmp/work/developer-1",
      },
    ]);
    w.git.removeOk = true;
    w.git.deleteOk = false;
    w.adapter.startError = new Error("again");
    await assert.rejects(w.launcher.spawn("developer"));
    assert.ok(eventNames(w).includes("branch_kept"));
  } finally {
    w.cleanup();
  }
});

test("a cleanup that cannot end the agent leaves it active, touches nothing else, and worker_limit names the reason", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    (w.core as unknown as { endAgent: () => never }).endAgent = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(w.launcher.spawn("developer"));
    assert.deepEqual(w.git.removed, []);
    assert.deepEqual(w.git.deleted, []);
    assert.equal(
      w.launcher.status().cleanupFailed[0]!.reason,
      "the seat still holds authority",
    );
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worker_limit" &&
        /still holds authority/.test(e.message),
    );
  } finally {
    w.cleanup();
  }
});

test("a seat of another kind or a disabled seat is refused before anything is created", async () => {
  const w = await world();
  try {
    await launched(w);
    w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer2-seat",
      name: "developer2",
      role: "Verifier",
    });
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "seat_kind_mismatch" &&
        /Verifier/.test(e.message),
    );
    assert.equal(
      w.core.listAgents().some((a) => a.roleName === "developer2"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("an orphan actor left by a crash on a reused seat is revoked before a new agent is created", async () => {
  const w = await world();
  try {
    await launched(w);
    w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const orphan = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "orphan",
      role: "Developer",
      seatId: "developer-seat",
    });
    assert.deepEqual(w.core.seatActorIds(w.owner, "developer-seat"), [
      orphan.actorId,
    ]);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    assert.ok(
      !w.core.seatActorIds(w.owner, "developer-seat").includes(orphan.actorId),
    );
  } finally {
    w.cleanup();
  }
});

test("operations run one at a time and a second waiting operation is answered busy at once", async () => {
  const w = await world();
  try {
    await launched(w);
    let release!: () => void;
    w.adapter.startGate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const first = w.launcher.spawn("developer");
    const second = w.launcher.spawn("developer2");
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) => e instanceof LauncherError && e.code === "busy",
    );
    assert.ok(
      !w.adapter.starts.some((s) => s.name.startsWith("developer")),
      "nothing started yet",
    );
    release();
    await Promise.all([first, second]);
    assert.equal(
      w.adapter.starts.filter((s) => s.name.startsWith("developer")).length,
      2,
    );
  } finally {
    w.cleanup();
  }
});

test("two concurrent spawns at a limit of one end with one agent and one refusal", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    const results = await Promise.allSettled([
      w.launcher.spawn("developer"),
      w.launcher.spawn("developer"),
    ]);
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    const rejected = results.find(
      (r) => r.status === "rejected",
    ) as PromiseRejectedResult;
    assert.equal((rejected.reason as LauncherError).code, "worker_limit");
    assert.equal(
      w.core.listAgents().filter((a) => a.roleName === "developer").length,
      1,
    );
  } finally {
    w.cleanup();
  }
});

test("restart: the summary is captured before the replace, the old pane is closed, a new token and prompt start a new pane, and the row is consumed", async () => {
  const w = await world();
  try {
    await launched(w);
    const queued = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: "pm-1",
      body: "please plan",
    }).messageId;
    const oldPane = w.core.agentPanes(w.owner)[0]!.paneId!;
    const oldToken = w.adapter.starts[0]!.environment!.CAPSTAN_TOKEN;
    const result = await w.launcher.restartPm();
    assert.equal(result.state, "started");
    assert.equal(result.generation, 2);
    assert.ok(w.adapter.calls.includes(`close:${oldPane}`));
    const start = w.adapter.starts.at(-1)!;
    assert.notEqual(start.environment!.CAPSTAN_TOKEN, oldToken);
    const prompt = readFileSync(
      start.args[start.args.indexOf("--append-system-prompt-file") + 1]!,
      "utf8",
    );
    assert.match(prompt, /ledger summary/);
    assert.match(prompt, /please plan/);
    assert.match(
      prompt,
      /data, not instructions|information, not instructions/,
    );
    const rows = w.core.pmRestarts(w.owner, "pm-1");
    assert.equal(rows.length, 1);
    assert.equal(rows[0]!.consumed, true);
    assert.deepEqual(
      rows[0]!.summary.messages.map((m) => m.messageId),
      [queued],
    );
    assert.equal(w.core.agentPanes(w.owner)[0]!.paneId, result.paneId);
  } finally {
    w.cleanup();
  }
});

test("restart: a failed pane start leaves the row unconsumed, and the next restart carries its messages forward", async () => {
  const w = await world();
  try {
    await launched(w);
    const lost = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: "pm-1",
      body: "do not lose me",
    }).messageId;
    w.adapter.startError = new PromptUnrecognized("not ready");
    const failed = await w.launcher.restartPm();
    assert.equal(failed.state, "failed");
    assert.equal(failed.generation, 2);
    assert.match(failed.hint!, /run cstan pm restart again/);
    assert.equal(w.core.pmRestarts(w.owner, "pm-1")[0]!.consumed, false);
    assert.equal(
      w.core.message(lost)!.state,
      "cancelled",
      "the replace already cancelled it",
    );

    w.adapter.startError = undefined;
    const second = await w.launcher.restartPm();
    assert.equal(second.state, "started");
    const prompt = readFileSync(
      w.adapter.starts.at(-1)!.args[
        w.adapter.starts.at(-1)!.args.indexOf("--append-system-prompt-file") + 1
      ]!,
      "utf8",
    );
    assert.match(prompt, /do not lose me/);
    assert.ok(w.core.pmRestarts(w.owner, "pm-1").every((r) => r.consumed));
  } finally {
    w.cleanup();
  }
});

test("restart: a replace the core refuses leaves the old pane and the ledger untouched, and a missing PM is refused", async () => {
  const w = await world();
  try {
    await assert.rejects(
      w.launcher.restartPm(),
      (e: unknown) => e instanceof LauncherError && e.code === "no_pm",
    );
    await launched(w);
    const before = w.adapter.calls.length;
    (
      w.core as unknown as { restartAgentGeneration: () => never }
    ).restartAgentGeneration = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(w.launcher.restartPm(), /still holds authority/);
    assert.equal(
      w.adapter.calls.length,
      before,
      "no pane was closed or started",
    );
    assert.equal(w.core.agentPanes(w.owner).length, 1);
  } finally {
    w.cleanup();
  }
});

test("restart: an old pane that cannot be closed is listed as an orphan and the new PM still starts", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const result = await w.launcher.restartPm();
    assert.equal(result.state, "started");
    assert.equal(w.launcher.status().orphanPanes.length, 1);
    assert.equal(w.launcher.status().orphanPanes[0]!.agentId, "pm-1");
  } finally {
    w.cleanup();
  }
});

test("after a daemon restart the recorded panes are re-registered, lost ones are cleared, a lost worker is ended and a lost PM needs a restart", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer2");
    const fresh = new StubAdapter();
    const second = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: "/tmp/x.sock",
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: { PATH: "/usr/bin" },
      git: w.git,
      log: (event, details) => w.events.push({ event, details }),
    });
    const rows = w.core.agentPanes(w.owner);
    const lostPane = rows.find((r) => r.agentId === "developer2-1")!.paneId!;
    fresh.adoptErrors.set(lostPane, new PaneGone("gone"));
    await second.adoptAll();
    assert.equal(fresh.paneForAgent("pm-1") !== undefined, true);
    assert.equal(fresh.paneForAgent("developer-1") !== undefined, true);
    assert.ok(fresh.calls.some((c) => c.startsWith("adopt-shell:")));
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer2-1")!.state,
      "ended",
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer2-1"),
      false,
    );
    assert.ok(eventNames(w).includes("pane_lost"));
    assert.equal(
      (await second.spawn("developer2")).agentId,
      "developer2-2",
      "the role is free again",
    );

    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const third = new StubAdapter();
    third.adoptErrors.set(pmPane, new AgentPaneMismatch("elsewhere"));
    const launcher = new Launcher({
      core: w.core,
      adapter: third,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const result = await launcher.launchPm();
    assert.equal(result.state, "needs_restart");
  } finally {
    w.cleanup();
  }
});

test("adoption sweeps stale rows and crashed spawns with and without a row, and keeps the row of a vanished watch pane", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    w.core.endAgent(ctx(w.core, w.owner), "developer-1");
    const crashedSeat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer2-seat",
      name: "developer2",
      role: "Developer",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "d2",
      role: "Developer",
      seatId: crashedSeat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "developer2-1",
      roleName: "developer2",
      seatId: crashedSeat.seatId,
      actorId: actor.actorId,
    });
    const fresh = new StubAdapter();
    const second = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
      log: (event, details) => w.events.push({ event, details }),
    });
    const fallback = w.core.fallbackPane(w.owner)!;
    fresh.adoptErrors.set(fallback.paneId, new PaneGone("gone"));
    await second.adoptAll();
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
      "the stale row is cleared",
    );
    assert.ok(
      fresh.calls.some((c) => c.startsWith("close:")),
      "the stale pane is closed",
    );
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer2-1")!.state,
      "ended",
      "a crashed spawn with no row is ended",
    );
    assert.deepEqual(
      w.core.fallbackPane(w.owner),
      fallback,
      "a gone watch pane keeps its row so the project workspace can get a new watch tab",
    );
    assert.ok(eventNames(w).includes("fallback_pane_gone"));
    assert.ok(eventNames(w).includes("crashed_spawn"));
  } finally {
    w.cleanup();
  }
});

test("a crashed spawn with an intent row removes its worktree found by branch and deletes the branch at the recorded sha", async () => {
  const w = await world();
  try {
    await launched(w);
    const seat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "d",
      role: "Developer",
      seatId: seat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "developer-1",
      roleName: "developer",
      seatId: seat.seatId,
      actorId: actor.actorId,
    });
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "developer-1",
      workspaceId: null,
      paneId: null,
      worktreePath: null,
      branch: "capstan/developer-1-g1",
      baseSha: SHA,
    });
    w.git.byBranch.set("capstan/developer-1-g1", "/tmp/found/by/branch");
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/found/by/branch"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer-1")!.state,
      "ended",
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("a project workspace that fails to open fails the PM start and leaves no agent, row or pane, and spawn needs a launched PM", async () => {
  const w = await world();
  try {
    w.adapter.runError = new Error("cannot run");
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason!, /project workspace could not be opened/);
    assert.equal(w.core.fallbackPane(w.owner), undefined);
    assert.equal(
      w.core.listAgents().filter((a) => a.state === "active").length,
      0,
    );
    assert.ok(
      w.adapter.calls.some((c) => c.startsWith("close:")),
      "the workspace made for the PM is closed again",
    );
    assert.ok(
      w.adapter.calls.some((c) => /^close:w1:p10\d$/.test(c)),
      "the watch tab's pane is closed again, so no tab is left unrecorded",
    );
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "pm_not_launched",
    );
  } finally {
    w.cleanup();
  }
});

test("the agent environment helper and the wrapper directory exist for every agent and the project files stay private", async () => {
  const w = await world();
  try {
    await launched(w);
    assert.ok(existsSync(path.join(w.root, ".capstan", "bin", "cstan")));
    mkdirSync(path.join(w.root, "x"), { recursive: true });
    const environment = buildAgentEnvironment(
      { PATH: "/p" },
      { PATH: "/a:/p" },
    );
    assert.equal(environment.PATH, "/a:/p");
  } finally {
    w.cleanup();
  }
});

test("an ended agent's leftover worktree and branch are released at the next start and its row is cleared", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    w.core.endAgent(ctx(w.core, w.owner), "developer-1");
    w.git.removed = [];
    w.git.deleted = [];
    await w.reopen().adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
    assert.deepEqual(w.launcher.status().cleanupFailed, []);
  } finally {
    w.cleanup();
  }
});

test("a worktree git refuses to remove keeps its row, is listed by status after a restart, and is retried until it goes", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    w.git.removeOk = false;
    await assert.rejects(w.launcher.spawn("developer"));
    const fresh = w.reopen();
    assert.deepEqual(fresh.status().cleanupFailed, [
      {
        agentId: "developer-1",
        reason: "git could not remove the worktree",
        worktreePath: "/tmp/work/developer-1",
      },
    ]);
    await fresh.adoptAll();
    assert.equal(
      fresh.status().cleanupFailed.length,
      1,
      "still refused, still listed",
    );
    w.git.removeOk = true;
    await fresh.adoptAll();
    assert.deepEqual(fresh.status().cleanupFailed, []);
    assert.deepEqual(w.git.deleted.at(-1), ["capstan/developer-1-g1", SHA]);
  } finally {
    w.cleanup();
  }
});

test("a restart closes the recorded old pane even when adoption did not register it, and lists it when it cannot be closed", async () => {
  const w = await world();
  try {
    await launched(w);
    const oldPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const fresh = new StubAdapter();
    const launcher = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    fresh.adoptErrors.set(oldPane, new HerdrError("timeout", "slow"));
    const result = await launcher.restartPm();
    assert.equal(result.state, "started");
    assert.ok(
      fresh.calls.includes(`close:${oldPane}`),
      "the recorded pane was closed",
    );

    const again = new StubAdapter();
    const second = new Launcher({
      core: w.core,
      adapter: again,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const current = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    again.adoptErrors.set(current, new HerdrError("timeout", "slow"));
    again.closeError = new HerdrError("pane_close_failed", "busy");
    const last = await second.restartPm();
    assert.equal(last.state, "started");
    assert.deepEqual(second.status().orphanPanes, [
      { agentId: "pm-1", paneId: current },
    ]);
    assert.ok(
      again.calls.includes(`forget:${current}`),
      "the old pane no longer blocks the agent name",
    );
  } finally {
    w.cleanup();
  }
});

test("a hub that cannot be re-adopted for a transient reason is not replaced by a second hub; a vanished watch pane is made again in the same workspace", async () => {
  const w = await world();
  try {
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    const fresh = new StubAdapter();
    const make = (adapter: StubAdapter) =>
      new Launcher({
        core: w.core,
        adapter,
        config: config(),
        projectRoot: w.root,
        cliPath: "/c.js",
        socketPath: "/s",
        credential: w.owner,
        git: w.git,
        baseEnvironment: { PATH: "/usr/bin" },
      });
    fresh.adoptErrors.set(hub.paneId, new HerdrError("timeout", "slow"));
    const launcher = make(fresh);
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    fresh.entries.set(pmPane, { agent: "pm-1" });
    fresh.agentPanes.set("pm-1", pmPane);
    await assert.rejects(
      launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "hub_unavailable",
    );
    assert.deepEqual(
      w.core.fallbackPane(w.owner),
      hub,
      "the recorded hub is untouched",
    );
    assert.ok(
      !fresh.calls.some((c) => c.startsWith("workspace:")),
      "no second hub",
    );

    fresh.adoptErrors.set(hub.paneId, new PaneGone("gone"));
    const result = await launcher.spawn("developer");
    assert.equal(result.state, "started");
    const remade = w.core.fallbackPane(w.owner)!;
    assert.equal(
      remade.workspaceId,
      hub.workspaceId,
      "a vanished watch pane is made again in the same project workspace",
    );
    assert.ok(
      fresh.calls.some((c) => c === `tab:${hub.workspaceId}:watch:worker`),
    );
    assert.ok(!fresh.calls.some((c) => c.startsWith("workspace:")));
  } finally {
    w.cleanup();
  }
});

test("status lists an unfinished cleanup once, forgets it when the agent has ended, and an orphan pane is retried and dropped once it closes", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    w.adapter.startError = new Error("start failed");
    (w.core as unknown as { endAgent: () => never }).endAgent = () => {
      throw new Error("the seat still holds authority");
    };
    await assert.rejects(w.launcher.spawn("developer"));
    await assert.rejects(w.launcher.spawn("developer"));
    assert.equal(
      w.launcher.status().cleanupFailed.length,
      1,
      "one entry per agent, not one per attempt",
    );

    w.adapter.startError = undefined;
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const r1 = await w.launcher.restartPm();
    const r2 = await w.launcher.restartPm();
    assert.equal(
      w.launcher.status().orphanPanes.length,
      2,
      JSON.stringify([
        r1,
        r2,
        w.launcher.status().orphanPanes,
        w.adapter.calls,
      ]),
    );
    w.adapter.closeError = undefined;
    await w.launcher.adoptAll();
    assert.deepEqual(
      w.launcher.status().orphanPanes,
      [],
      "closed orphans are dropped",
    );
  } finally {
    w.cleanup();
  }
});

test("a leftover row without a worktree path is reported as a record waiting to be cleaned up", async () => {
  const w = await world();
  try {
    await launched(w);
    const seat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "d",
      role: "Developer",
      seatId: seat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "developer-1",
      roleName: "developer",
      seatId: seat.seatId,
      actorId: actor.actorId,
    });
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "developer-1",
      workspaceId: "w9",
      paneId: "w9:p1",
      worktreePath: null,
      branch: null,
      baseSha: null,
    });
    w.core.endAgent(ctx(w.core, w.owner), "developer-1");
    assert.deepEqual(w.launcher.status().cleanupFailed, [
      {
        agentId: "developer-1",
        reason: "a record of an ended agent is waiting to be cleaned up",
      },
    ]);
  } finally {
    w.cleanup();
  }
});

test("a pane Herdr no longer knows counts as closed, never as an orphan", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.closeError = new HerdrError("pane_not_found", "no such pane");
    const result = await w.launcher.restartPm();
    assert.equal(result.state, "started");
    assert.deepEqual(w.launcher.status().orphanPanes, []);
  } finally {
    w.cleanup();
  }
});

test("a project path with a colon is refused because cstan could not be found on PATH", async () => {
  const w = await world();
  try {
    const odd = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: config(),
      projectRoot: path.join(w.root, "a:b"),
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const result = await odd.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason!, /colon/);
    assert.equal(
      w.core.listAgents().filter((a) => a.state === "active").length,
      0,
    );
  } finally {
    w.cleanup();
  }
});

test("a SHA-256 repository is named as unsupported instead of reported as having no commit", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-sha256-"));
  try {
    const init = spawnSync(
      "git",
      ["init", "-q", "--object-format=sha256", root],
      { encoding: "utf8" },
    );
    if (init.status !== 0) return; // this git cannot make one; nothing to check
    spawnSync(
      "git",
      [
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@example.com",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "x",
      ],
      { cwd: root },
    );
    assert.throws(
      () => defaultGit(root).headSha(),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "git_error" &&
        /SHA-256/.test(e.message),
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("an orphan pane is kept in the ledger, so a new launcher after a daemon restart still lists and retries it", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    await w.launcher.restartPm();
    const pane = w.launcher.status().orphanPanes[0]!.paneId;
    const afterRestart = w.reopen();
    assert.deepEqual(afterRestart.status().orphanPanes, [
      { agentId: "pm-1", paneId: pane },
    ]);
    w.adapter.closeError = undefined;
    await afterRestart.adoptAll();
    assert.deepEqual(afterRestart.status().orphanPanes, []);
    assert.ok(w.adapter.calls.includes(`close:${pane}`));
  } finally {
    w.cleanup();
  }
});

test("when git cannot say whether a worktree exists the branch and the row are kept, never deleted", async () => {
  const w = await world();
  try {
    await launched(w);
    const seat = w.core.createSeat(ctx(w.core, w.owner), {
      seatId: "developer-seat",
      name: "developer",
      role: "Developer",
    });
    const actor = w.core.createActor(ctx(w.core, w.owner), {
      displayName: "d",
      role: "Developer",
      seatId: seat.seatId,
    });
    w.core.registerAgent(ctx(w.core, w.owner), {
      agentId: "developer-1",
      roleName: "developer",
      seatId: seat.seatId,
      actorId: actor.actorId,
    });
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "developer-1",
      workspaceId: null,
      paneId: null,
      worktreePath: null,
      branch: "capstan/developer-1-g1",
      baseSha: SHA,
    });
    w.git.byBranchError = new LauncherError(
      "git_error",
      "git could not list the worktrees",
    );
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.deleted, []);
    assert.deepEqual(w.git.removed, []);
    assert.ok(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      "the row stays",
    );
    assert.ok(eventNames(w).includes("worktree_unknown"));
    w.git.byBranchError = undefined;
    w.git.byBranch.set("capstan/developer-1-g1", "/tmp/found");
    await w.launcher.adoptAll();
    assert.deepEqual(w.git.removed, ["/tmp/found"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("variables listed in env.pass reach the PM, every worker and every reviewer, and nothing else does", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      pass: ["NEXORA_API_KEY", "EXTRA_SETTING"],
      base: {
        NEXORA_API_KEY: "key-value-1",
        EXTRA_SETTING: "two words",
        UNLISTED: "no",
      },
    },
  );
  try {
    await w.launcher.launchPm();
    await w.launcher.spawn("developer");
    await w.launcher.spawn("developer2", { baseSha: "a".repeat(40) });
    assert.equal(w.adapter.starts.length, 3);
    for (const start of w.adapter.starts) {
      assert.equal(start.environment!.NEXORA_API_KEY, "key-value-1");
      assert.equal(start.environment!.EXTRA_SETTING, "two words");
      assert.equal(start.environment!.UNLISTED, undefined);
      assert.equal(start.environment!.SECRET, undefined);
    }
    assert.equal(
      w.adapter.lastShellEnvironment?.NEXORA_API_KEY,
      undefined,
      "the watch pane does not get passed variables",
    );
    const logged = JSON.stringify(w.events);
    assert.ok(
      !logged.includes("key-value-1"),
      "a passed value is never logged",
    );
  } finally {
    w.cleanup();
  }
});

test("a listed variable that is not set is named in the answer and the log, and the others still pass", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      pass: ["NEXORA_API_KEY", "MISSING_ONE", "EMPTY_ONE"],
      base: { NEXORA_API_KEY: "key-value-2", EMPTY_ONE: "" },
    },
  );
  try {
    const launched = await w.launcher.launchPm();
    assert.deepEqual(launched.missingEnv, ["MISSING_ONE", "EMPTY_ONE"]);
    assert.match(
      launched.warning ?? "",
      /^MISSING_ONE, EMPTY_ONE are listed in \[env\] pass but not set where the daemon was started/,
    );
    const spawned = await w.launcher.spawn("developer");
    assert.deepEqual(spawned.missingEnv, ["MISSING_ONE", "EMPTY_ONE"]);
    assert.equal(w.adapter.starts[1]!.environment!.EMPTY_ONE, undefined);
    const again = await w.launcher.launchPm();
    assert.equal(again.state, "running");
    assert.equal(again.missingEnv, undefined, "nothing was started");
    assert.equal(again.warning, undefined);
    const restarted = await w.launcher.restartPm();
    assert.deepEqual(restarted.missingEnv, ["MISSING_ONE", "EMPTY_ONE"]);
    assert.equal(w.adapter.starts[1]!.environment!.MISSING_ONE, undefined);
    assert.equal(
      w.adapter.starts[1]!.environment!.NEXORA_API_KEY,
      "key-value-2",
    );
    assert.ok(
      w.events.some(
        (e) =>
          e.event === "env_pass_missing" &&
          JSON.stringify(e.details) === '{"names":["MISSING_ONE","EMPTY_ONE"]}',
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("with nothing listed the answers carry no missingEnv and an unacceptable passed value fails naming only the variable", async () => {
  const plain = await world();
  try {
    assert.equal((await plain.launcher.launchPm()).missingEnv, undefined);
  } finally {
    plain.cleanup();
  }
  const bad = await world(
    true,
    true,
    3,
    {},
    {
      pass: ["BAD_VALUE", "ALSO_MISSING"],
      base: { BAD_VALUE: "secret\u0007text" },
    },
  );
  try {
    const result = await bad.launcher.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason ?? "", /BAD_VALUE/);
    assert.equal(
      result.missingEnv,
      undefined,
      "a failed launch started nothing",
    );
    assert.ok(!JSON.stringify(result).includes("secret"));
    assert.ok(!JSON.stringify(bad.events).includes("secret"));
  } finally {
    bad.cleanup();
  }
});

test("overlapping operations are told apart: only the one that started an agent reports the missing variable", async () => {
  const w = await world(true, true, 3, {}, { pass: ["MISSING_ONE"], base: {} });
  try {
    const [first, second] = await Promise.all([
      w.launcher.launchPm(),
      w.launcher.launchPm(),
    ]);
    assert.equal(first.state, "started");
    assert.deepEqual(first.missingEnv, ["MISSING_ONE"]);
    assert.equal(second.state, "running");
    assert.equal(second.missingEnv, undefined);
    assert.equal(second.warning, undefined);
    assert.equal(
      w.events.filter((e) => e.event === "env_pass_missing").length,
      1,
    );
  } finally {
    w.cleanup();
  }
});

test("observe reads the recorded pane of an active agent, sanitizes the text and reports Herdr's state; an ended or paneless agent is refused", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const spawned = await w.launcher.spawn("developer");
    const paneId = w.core
      .agentPanes(w.owner)
      .find((row) => row.agentId === spawned.agentId)!.paneId!;
    w.adapter.screens.set(
      paneId,
      "\u001b[31mnpm test\u001b[0m\r\nFAIL expected 3 got 4\u0007",
    );
    w.adapter.observation = "working";
    const seen = await w.launcher.observe(spawned.agentId, 25);
    assert.equal(seen.text, "npm test\nFAIL expected 3 got 4");
    assert.equal(seen.agentStatus, "working");
    assert.equal(seen.kind, "Developer");
    assert.equal(seen.roleName, "developer");
    assert.deepEqual(w.adapter.screenReads.at(-1), { paneId, lines: 25 });
    w.adapter.unreadablePanes.add(paneId);
    await assert.rejects(
      w.launcher.observe(spawned.agentId, 25),
      (error: Error) =>
        error instanceof LauncherError && error.code === "pane_unreadable",
    );
    w.adapter.unreadablePanes.delete(paneId);
    await assert.rejects(
      w.launcher.observe("nobody", 40),
      (error: Error) =>
        error instanceof LauncherError && error.code === "agent_not_active",
    );
    await w.launcher.release(spawned.agentId);
    await assert.rejects(
      w.launcher.observe(spawned.agentId, 40),
      (error: Error) =>
        error instanceof LauncherError && error.code === "agent_not_active",
    );
  } finally {
    w.cleanup();
  }
});

test("capturePrompt and answerPrompt resolve the active agent's pane, pass the hash and answer through, log the keys, and refuse an agent that is not active", async () => {
  const w = await world();
  try {
    await w.launcher.launchPm();
    const spawned = await w.launcher.spawn("developer");
    const paneId = w.core
      .agentPanes(w.owner)
      .find((row) => row.agentId === spawned.agentId)!.paneId!;
    assert.deepEqual(await w.launcher.capturePrompt(spawned.agentId), {
      captured: false,
      reason: "prompt_unrecognized",
    });
    assert.ok(w.adapter.calls.includes(`capture:${paneId}`));
    let before = 0;
    const outcome = await w.launcher.answerPrompt(spawned.agentId, {
      promptSha: "a".repeat(64),
      answer: { kind: "option", number: 1 },
      beforeType: () => {
        before += 1;
      },
    });
    assert.deepEqual(outcome, { typed: true, keys: ["enter"] });
    assert.equal(before, 1);
    assert.deepEqual(w.adapter.answered, [
      {
        paneId,
        promptSha: "a".repeat(64),
        answer: { kind: "option", number: 1 },
      },
    ]);
    for (const call of [
      () => w.launcher.capturePrompt("nobody"),
      () =>
        w.launcher.answerPrompt("nobody", {
          promptSha: "a".repeat(64),
          answer: { kind: "esc" },
          beforeType: () => undefined,
        }),
    ])
      await assert.rejects(
        call(),
        (error: Error) =>
          error instanceof LauncherError && error.code === "agent_not_active",
      );
    assert.equal(w.adapter.answered.length, 1);
  } finally {
    w.cleanup();
  }
});

/** An accepted report by the worker, using the token it was started with. */
function reportAs(
  w: World,
  agentId: string,
  startIndex: number,
  commit: string,
  summary: string,
): void {
  const token = w.adapter.starts[startIndex]!.environment!.CAPSTAN_TOKEN!;
  const row = w.core.agentPanes(w.owner).find((r) => r.agentId === agentId)!;
  const { record } = w.core.recordAgentReport(ctx(w.core, token), {
    commitSha: commit,
    summary,
    evidence: {
      generation: 1,
      branch: row.branch!,
      baseSha: row.baseSha!,
      commitExists: true,
      branchTip: commit,
      isAncestorOfTip: true,
      isAncestorOfBase: false,
      checkedAt: "2026-10-01T00:00:00.000Z",
    },
  });
  assert.equal(record.state, "accepted");
}

function promptOf(w: World, startIndex: number): string {
  const args = w.adapter.starts[startIndex]!.args;
  return readFileSync(
    args[args.indexOf("--append-system-prompt-file") + 1]!,
    "utf8",
  );
}

test("replace releases a running worker and starts a new agent of the same role from its last accepted report, seeded from the ledger, and sends nothing again", async () => {
  const w = await world();
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    const sent = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: old.agentId,
      body: "write the parser",
    }).messageId;
    const commit = "c".repeat(40);
    w.git.reachable.add(commit);
    w.git.tips.set(old.branch, "d".repeat(40));
    reportAs(w, old.agentId, 1, commit, "parser written");
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(result.predecessor, old.agentId);
    assert.equal(result.baseSha, commit);
    assert.equal(result.baseSource, "predecessor");
    assert.equal(result.replacementRecorded, true);
    assert.deepEqual(result.cancelledMessageIds, [sent]);
    assert.notEqual(result.agentId, old.agentId, "a new id");
    assert.equal(w.core.agentRecord(old.agentId)!.state, "ended");
    assert.equal(w.core.agentRecord(result.agentId)!.state, "active");
    assert.equal(w.core.agentRecord(result.agentId)!.roleName, "developer");
    assert.equal(w.core.isAgentReplaced(old.agentId), true);
    assert.deepEqual(
      w.core.messagesFor(result.agentId),
      [],
      "nothing is sent to the replacement",
    );
    assert.equal(w.core.message(sent)!.state, "cancelled");
    const prompt = promptOf(w, 2);
    assert.match(
      prompt,
      /===== replacement seed, generated from the ledger =====/,
    );
    assert.ok(
      prompt.includes(
        `You replace agent ${old.agentId} (role developer), which has ended`,
      ),
    );
    assert.ok(
      prompt.includes(
        `Your branch starts at ${commit}, the predecessor's last accepted report`,
      ),
    );
    assert.ok(prompt.includes(`(tip ${"d".repeat(40)})`), prompt);
    assert.ok(prompt.includes(JSON.stringify("write the parser")));
    assert.ok(
      prompt.includes(
        `commit ${commit} on ${old.branch}: ${JSON.stringify("parser written")}`,
      ),
    );
    assert.ok(prompt.includes(`${sent} from operator [queued]`));
    await assert.rejects(
      w.launcher.replace(old.agentId),
      (error: Error) =>
        error instanceof LauncherError && error.code === "already_replaced",
    );
  } finally {
    w.cleanup();
  }
});

test("replace moves the work packages of the predecessor to the replacement and the seed names them", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    const old = await w.launcher.spawn("developer");
    const other = await w.launcher.spawn("developer");
    const planId = w.core.openPlan(ctx(w.core, w.owner), {
      tier: "normal",
      title: "split",
    }).planId;
    w.core.submitPlan(
      ctx(w.core, w.adapter.starts[1]!.environment!.CAPSTAN_TOKEN!),
      {
        planId,
        bodyJson: JSON.stringify({
          summary: "s",
          packages: [
            {
              id: "wp1",
              title: "parser",
              owns: ["src/parser.ts"],
              acceptance: ["parses empty input"],
            },
            { id: "wp2", title: "other" },
          ],
        }),
        baseSha: "a".repeat(40),
        review: false,
      },
    );
    for (const packageId of ["wp1", "wp2"])
      w.core.assignPackage(ctx(w.core, w.owner), {
        planId,
        packageId,
        agentId: packageId === "wp1" ? old.agentId : other.agentId,
      });
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    const packages = w.core.planRecord(w.owner, planId)!.packages;
    assert.equal(
      packages.find((p) => p.packageId === "wp1")!.assigneeAgentId,
      result.agentId,
    );
    assert.equal(
      packages.find((p) => p.packageId === "wp2")!.assigneeAgentId,
      other.agentId,
      "a package of another agent is not touched",
    );
    const prompt = promptOf(w, 4);
    assert.ok(
      prompt.includes(
        `${planId}/wp1: "parser"; owns "src/parser.ts"; acceptance "parses empty input"`,
      ),
      prompt,
    );
    assert.ok(!prompt.includes(`${planId}/wp2`));
  } finally {
    w.cleanup();
  }
});

test("replace falls back to the project's HEAD when the predecessor has no accepted report, its commit is not reachable or its branch is gone", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const a = await w.launcher.replace(first.agentId);
    assert.equal(a.state, "started");
    if (a.state !== "started") return;
    assert.equal(a.baseSource, "head");
    assert.equal(a.baseSha, w.git.head);
    assert.match(
      promptOf(w, 2),
      /the project's HEAD \(the predecessor had no accepted report that could be used\)/,
    );
    const commit = "e".repeat(40);
    reportAs(w, a.agentId, 2, commit, "work");
    w.git.reachable.clear();
    const b = await w.launcher.replace(a.agentId);
    assert.equal(b.state, "started");
    if (b.state !== "started") return;
    assert.equal(b.baseSource, "head", "an unreachable commit is not used");
    w.git.reachable.add(commit);
    const c = await w.launcher.replace(b.agentId);
    assert.equal(
      c.state === "started" && c.baseSource,
      "head",
      "no report by that agent",
    );
  } finally {
    w.cleanup();
  }
});

test("replace seeds from the head with the predecessor tip when the last accepted commit was amended away", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const tip = "d".repeat(40);
    w.git.tips.set(first.branch, tip);
    reportAs(w, first.agentId, 1, "e".repeat(40), "work");
    w.git.reachable.clear();
    const result = await w.launcher.replace(first.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(result.baseSource, "head");
    assert.equal(result.baseSha, w.git.head);
    assert.ok(promptOf(w, 2).includes(`(tip ${tip})`));
  } finally {
    w.cleanup();
  }
});

test("replace works on an agent that already ended, releases nothing and answers with no predecessor worktree", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    const removedBefore = [...w.git.removed];
    const result = await w.launcher.replace(first.agentId);
    assert.equal(result.state, "started");
    if (result.state !== "started") return;
    assert.equal(result.predecessorWorktreeRemoved, null);
    assert.deepEqual(result.cancelledMessageIds, []);
    assert.deepEqual(w.git.removed, removedBefore);
    assert.ok(
      promptOf(w, 2).includes(
        `You replace agent ${first.agentId} (role developer), which has ended`,
      ),
    );
  } finally {
    w.cleanup();
  }
});

test("replace stops when the old pane will not close, so two agents never work on one task, and a pane that is already gone is fine", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    const blocked = await w.launcher.replace(first.agentId);
    assert.equal(blocked.state, "blocked");
    assert.ok("reason" in blocked);
    if ("reason" in blocked)
      assert.match(
        blocked.reason,
        new RegExp(
          `${first.agentId} was released but its pane is still open: close it in Herdr, then run cstan replace ${first.agentId} again`,
        ),
      );
    assert.equal(w.adapter.starts.length, 2, "no replacement was started");
    assert.equal(w.core.isAgentReplaced(first.agentId), false);
    w.adapter.closeError = undefined;
    const second = await w.launcher.spawn("developer");
    w.adapter.closeMissingThrows = true;
    w.adapter.entries.delete(second.paneId);
    const result = await w.launcher.replace(second.agentId);
    assert.equal(
      result.state,
      "started",
      "a pane that is already gone is not an error",
    );
  } finally {
    w.cleanup();
  }
});

test("replace refuses the PM, an unknown agent, a running integration, a second replacement and a concurrent one, before anything is released", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await assert.rejects(
      w.launcher.replace("pm-1"),
      (e: Error) =>
        e instanceof LauncherError && e.code === "kind_not_replaceable",
    );
    await assert.rejects(
      w.launcher.replace("nobody"),
      (e: Error) => e instanceof LauncherError && e.code === "unknown_agent",
    );
    assert.ok(first);
  } finally {
    w.cleanup();
  }
  const v = await world();
  try {
    await launched(v);
    const target = await v.launcher.spawn("developer");
    const original = v.core.runningIntegrations.bind(v.core);
    v.core.runningIntegrations = () => [{ integrationId: "x" } as never];
    await assert.rejects(
      v.launcher.replace(target.agentId),
      (e: Error) =>
        e instanceof LauncherError && e.code === "integration_running",
    );
    assert.equal(
      v.core.agentRecord(target.agentId)!.state,
      "active",
      "nothing was released",
    );
    v.core.runningIntegrations = original;
    const gate = v.launcher.replace(target.agentId);
    await assert.rejects(
      v.launcher.replace(target.agentId),
      (e: Error) => e instanceof LauncherError && e.code === "replace_running",
    );
    assert.equal((await gate).state, "started");
  } finally {
    v.cleanup();
  }
});

test("replace names the released agent when the replacement cannot start, and a rerun finds it ended and cancels nothing twice", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    const queued = w.core.enqueueMessage(ctx(w.core, w.owner), {
      recipientAgentId: first.agentId,
      body: "x",
    }).messageId;
    w.git.branchNamesValid = false;
    await assert.rejects(
      w.launcher.replace(first.agentId),
      (e: Error) =>
        e instanceof LauncherError &&
        e.code === "replacement_not_started" &&
        e.message.includes(
          `${first.agentId} was released but the replacement could not start`,
        ) &&
        e.message.includes(`run cstan replace ${first.agentId} again`),
    );
    assert.equal(w.core.agentRecord(first.agentId)!.state, "ended");
    assert.equal(w.core.message(queued)!.state, "cancelled");
    assert.equal(w.core.isAgentReplaced(first.agentId), false);
    w.git.branchNamesValid = true;
    const rerun = await w.launcher.replace(first.agentId);
    assert.equal(rerun.state, "started");
    assert.equal(w.core.isAgentReplaced(first.agentId), true);
  } finally {
    w.cleanup();
  }
});

test("a worker on a Codex host starts with full access, its worktree trusted and the prompt as instructions", async () => {
  const w = await world(true, true, 3, {}, { hostOf: { developer: "codex" } });
  mkdirSync("/tmp/work/developer-1", { recursive: true });
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    const start = w.adapter.starts.find((s) => s.name === "developer-1")!;
    assert.equal(start.kind, "codex");
    assert.deepEqual(start.args.slice(0, 4), [
      "--sandbox",
      "danger-full-access",
      "--ask-for-approval",
      "never",
    ]);
    assert.ok(!start.args.includes(CSTAN_ALLOW_RULE));
    assert.ok(!start.args.includes("--append-system-prompt-file"));
    const real = realpathSync("/tmp/work/developer-1");
    assert.ok(start.args.includes(`projects."${real}".trust_level="trusted"`));
    const instructions = start.args.find((a) =>
      a.startsWith("developer_instructions="),
    )!;
    assert.match(instructions, /cstan ack <message-id>/);
    assert.ok(!instructions.includes("\n"));
    assert.equal(w.adapter.starts[0]!.kind, "claude", "the PM stays on Claude");
  } finally {
    w.cleanup();
  }
});

test("a worker on an OMP host starts with every tool approved and the prompt file appended", async () => {
  const w = await world(true, true, 3, {}, { hostOf: { developer: "omp" } });
  try {
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.equal(result.state, "started");
    const start = w.adapter.starts.find((s) => s.name === "developer-1")!;
    assert.equal(start.kind, "omp");
    assert.deepEqual(start.args.slice(0, 2), ["--approval-mode", "yolo"]);
    const file = start.args[start.args.indexOf("--append-system-prompt") + 1]!;
    assert.match(readFileSync(file, "utf8"), /cstan ack <message-id>/);
    assert.ok(!start.args.includes(CSTAN_ALLOW_RULE));
  } finally {
    w.cleanup();
  }
});

test("workspaces are labelled with the project and every started pane reports project, role and agent", async () => {
  const w = await world();
  try {
    const project = path.basename(w.root);
    await launched(w);
    const spawned = await w.launcher.spawn("developer");
    assert.equal(spawned.state, "started");
    assert.deepEqual(
      w.adapter.calls.filter((c) => c.startsWith("workspace:")),
      [`workspace:${project}:PM`],
      "one workspace per project, named after it, whose root pane is the PM's",
    );
    const hub = w.core.fallbackPane(w.owner)!;
    assert.ok(
      w.adapter.calls.includes(`tab:${hub.workspaceId}:watch:worker`),
      "the watch shell is a tab of that workspace",
    );
    assert.equal(
      w.core.agentPanes(w.owner).find((r) => r.agentId === "pm-1")!.workspaceId,
      hub.workspaceId,
      "the PM lives in the project workspace",
    );
    assert.ok(w.adapter.created.includes(`${project} · developer-1`));
    assert.ok(w.adapter.labels.includes(`${hub.workspaceId}:${project}`));
    assert.ok(
      w.adapter.labels.includes(`${hub.workspaceId}:t1:pm`),
      "the PM's tab is named pm",
    );
    const pm = w.adapter.metadata.find((m) => m.tokens.agent === "pm-1")!;
    assert.deepEqual(pm.tokens, {
      project,
      role: "pm",
      agent: "pm-1",
    });
    const dev = w.adapter.metadata.find(
      (m) => m.tokens.agent === "developer-1",
    )!;
    assert.deepEqual(dev.tokens, {
      project,
      role: "developer",
      agent: "developer-1",
    });
    assert.equal(
      w.adapter.metadata.filter((m) => m.tokens.agent === undefined).length,
      2,
      "the PM's and the watch workspace carry the project token; a worker adds none",
    );
  } finally {
    w.cleanup();
  }
});

test("a failure to report metadata is logged and never fails a start", async () => {
  const w = await world();
  try {
    w.adapter.metadataError = new Error("herdr is busy");
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "started");
    assert.ok(w.events.some((e) => e.event === "describe_failed"));
  } finally {
    w.cleanup();
  }
});

test("an adopted PM keeps the project workspace name, an old PM workspace gets its project label, and metadata is reported again", async () => {
  const w = await world();
  try {
    const project = path.basename(w.root);
    await launched(w);
    const spawned = await w.launcher.spawn("developer");
    assert.equal(spawned.state, "started");
    w.adapter.labels.length = 0;
    w.adapter.metadata.length = 0;
    w.adapter.entries.clear();
    w.adapter.agentPanes.clear();
    await w.reopen().adoptAll();
    const worker = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "developer-1")!;
    assert.ok(
      !w.adapter.labels.some((l) => l.endsWith("developer-1")),
      "a worker's workspace may be the PM's, so it is never relabelled",
    );
    assert.ok(worker.workspaceId !== null);
    const pm = w.core.agentPanes(w.owner).find((r) => r.agentId === "pm-1")!;
    const hub = w.core.fallbackPane(w.owner)!;
    assert.equal(pm.workspaceId, hub.workspaceId);
    assert.ok(
      !w.adapter.labels.includes(`${pm.workspaceId}:${project} · pm`),
      "a PM in the project workspace never renames it to a pm label",
    );
    assert.ok(w.adapter.metadata.some((m) => m.tokens.agent === "developer-1"));

    // A PM started before the project workspace existed still has a workspace of its own.
    w.core.recordAgentPane(ctx(w.core, w.owner), {
      agentId: "pm-1",
      workspaceId: "w77",
      paneId: pm.paneId,
      worktreePath: null,
      branch: null,
      baseSha: null,
    });
    w.adapter.entries.clear();
    w.adapter.agentPanes.clear();
    w.adapter.labels.length = 0;
    await w.reopen().adoptAll();
    assert.ok(w.adapter.labels.includes(`w77:${project} · pm`));
  } finally {
    w.cleanup();
  }
});

test("adoption renames a watch workspace of an earlier layout to the project name", async () => {
  const w = await world();
  try {
    const project = path.basename(w.root);
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    w.adapter.entries.clear();
    w.adapter.agentPanes.clear();
    w.adapter.labels.length = 0;
    await w.reopen().adoptAll();
    assert.ok(w.adapter.labels.includes(`${hub.workspaceId}:${project}`));
  } finally {
    w.cleanup();
  }
});

test("a hub made again while a PM is already running closes its empty root pane, and keeps the watch tab", async () => {
  const w = await world();
  try {
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const fresh = new StubAdapter();
    fresh.adoptErrors.set(hub.paneId, new PaneGone("gone"));
    fresh.tabErrors.add(hub.workspaceId!);
    await fresh.createWorkspace({ cwd: "/x", label: "other", role: "worker" });
    fresh.calls.length = 0;
    fresh.entries.set(pmPane, { agent: "pm-1" });
    fresh.agentPanes.set("pm-1", pmPane);
    const launcher = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const result = await launcher.launchPm();
    assert.equal(result.state, "running");
    assert.equal(result.hub, "opened");
    const made = w.core.fallbackPane(w.owner)!;
    assert.notEqual(made.workspaceId, hub.workspaceId);
    const root = fresh.calls.find((c) => c.startsWith("workspace:"))!;
    assert.ok(root.endsWith(":PM"));
    assert.ok(
      fresh.calls.some((c) => c === `close:${made.workspaceId}:p1`),
      "the empty root pane is closed",
    );
    assert.ok(
      fresh.calls.some((c) => c === `tab:${made.workspaceId}:watch:worker`),
    );
  } finally {
    w.cleanup();
  }
});

test("a watch tab that cannot be made again for a transient reason keeps the row and makes no second hub", async () => {
  const w = await world();
  try {
    await launched(w);
    const hub = w.core.fallbackPane(w.owner)!;
    const pmPane = w.core
      .agentPanes(w.owner)
      .find((r) => r.agentId === "pm-1")!.paneId!;
    const fresh = new StubAdapter();
    fresh.adoptErrors.set(hub.paneId, new PaneGone("gone"));
    fresh.tabFailure = new HerdrError("timeout", "slow");
    fresh.entries.set(pmPane, { agent: "pm-1" });
    fresh.agentPanes.set("pm-1", pmPane);
    const launcher = new Launcher({
      core: w.core,
      adapter: fresh,
      config: config(),
      projectRoot: w.root,
      cliPath: "/c.js",
      socketPath: "/s",
      credential: w.owner,
      git: w.git,
      baseEnvironment: { PATH: "/usr/bin" },
    });
    const result = await launcher.launchPm();
    assert.equal(result.hub, "failed");
    assert.deepEqual(w.core.fallbackPane(w.owner), hub, "the row is kept");
    assert.ok(!fresh.calls.some((c) => c.startsWith("workspace:")));
  } finally {
    w.cleanup();
  }
});

test("a PM start that fails before it takes the new workspace's root pane closes that pane", async () => {
  const w = await world();
  try {
    w.adapter.promptError = new Error("disk full");
    const result = await w.launcher.launchPm();
    assert.equal(result.state, "failed");
    assert.match(result.reason!, /disk full/);
    assert.equal(
      w.adapter.calls.filter((c) => c === "close:w1:p1").length,
      1,
      "the empty root pane is closed once",
    );
    assert.ok(w.core.fallbackPane(w.owner), "the watch tab stays recorded");
  } finally {
    w.cleanup();
  }
});

test("the Supervisor does not take a worker's place: it starts when every worker place is taken, and is not counted against the limit", async () => {
  const w = await world(true, true, 1);
  try {
    await launched(w);
    const dev = await w.launcher.spawn("developer");
    assert.equal(dev.state, "started");
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) => e instanceof LauncherError && e.code === "worker_limit",
    );
    const supervisor = await w.launcher.spawn("supervisor");
    assert.equal(supervisor.state, "started");
    await assert.rejects(
      w.launcher.spawn("developer2"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worker_limit" &&
        /1 of 1 workers are active \(developer-1\)/.test(e.message),
      "the Supervisor is not listed or counted among the workers",
    );
  } finally {
    w.cleanup();
  }
});

test("the Architect takes no worker place unless count_toward_worker_limit is true, and its prompt differs from a developer's", async () => {
  const exempt = await world(
    true,
    true,
    1,
    {},
    { architect: { counts: false } },
  );
  try {
    await launched(exempt);
    const architect = await exempt.launcher.spawn("architect");
    assert.equal(architect.state, "started");
    const dev = await exempt.launcher.spawn("developer");
    assert.equal(dev.state, "started", "the developer still has its place");
    await assert.rejects(
      exempt.launcher.spawn("developer2"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worker_limit" &&
        /1 of 1 workers are active \(developer-1\)/.test(e.message),
    );
    const prompts = exempt.adapter.prompts;
    assert.ok(prompts.some((t) => t.includes("You are the architect")));
    assert.ok(
      prompts.some((t) => t.includes("the architect named in it can answer")),
    );
    assert.ok(
      prompts.some((t) => t.includes("Planned work")),
      "the PM prompt carries the plan section",
    );
  } finally {
    exempt.cleanup();
  }
  const counted = await world(
    true,
    true,
    1,
    {},
    { architect: { counts: true } },
  );
  try {
    await launched(counted);
    await counted.launcher.spawn("architect");
    await assert.rejects(
      counted.launcher.spawn("developer"),
      (e: unknown) => e instanceof LauncherError && e.code === "worker_limit",
    );
  } finally {
    counted.cleanup();
  }
});

test("without the Architect no prompt mentions plans", async () => {
  const w = await world();
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    for (const text of w.adapter.prompts) {
      assert.ok(!/architect/i.test(text), "no architect text");
      assert.ok(!text.includes("cstan plan"), "no plan commands");
    }
  } finally {
    w.cleanup();
  }
});

test("an enabled Researcher gets the researcher prompt and the MCP arguments, and the PM prompt gains the research section", async () => {
  const w = await world(true, true, 3, {}, { researcher: { enabled: true } });
  try {
    await launched(w);
    const researcher = await w.launcher.spawn("researcher");
    assert.equal(researcher.state, "started");
    const start = w.adapter.starts.at(-1)!;
    assert.ok(start.args.includes("--mcp-config"));
    assert.ok(start.args.includes("--strict-mcp-config"));
    assert.ok(start.args.includes(CSTAN_ALLOW_RULE));
    const prompt = promptOf(w, w.adapter.starts.length - 1);
    assert.match(prompt, /You are the researcher of a Capstan delivery team/);
    assert.match(prompt, /docs\/research\//);
    assert.match(prompt, /capstan-researcher\/1\.0 \(test\)/);
    const pm = promptOf(w, 0);
    assert.match(pm, /Web research \(the Researcher is enabled/);
    await w.launcher.spawn("developer");
    const dev = promptOf(w, w.adapter.starts.length - 1);
    assert.ok(!dev.includes("You are the researcher"));
    assert.ok(!dev.includes("Web research"));
  } finally {
    w.cleanup();
  }
});

test("the cstan allow rule the launcher appends passes the researcher rule check", () => {
  const role = {
    name: "researcher",
    kind: "Developer",
    permissionMode: "default",
    allow: [CSTAN_ALLOW_RULE],
    deny: [...RESEARCHER_REQUIRED_DENY],
  } as unknown as ResolvedRole;
  assert.deepEqual(
    researcherRuleProblems(role, {
      enabled: true,
      role: "researcher",
      outputDir: "docs/research",
    } as unknown as ResolvedResearcher),
    [],
  );
});

test("with [researcher] disabled a role named researcher gets the plain developer prompt and the PM has no research section", async () => {
  const w = await world(true, true, 3, {}, { researcher: { enabled: false } });
  try {
    await launched(w);
    await w.launcher.spawn("researcher");
    const prompt = promptOf(w, w.adapter.starts.length - 1);
    assert.ok(!prompt.includes("You are the researcher"));
    assert.ok(!prompt.includes("Output contract"));
    assert.ok(!/Web research/.test(promptOf(w, 0)));
  } finally {
    w.cleanup();
  }
});

test("replace and PM restart rebuild the researcher prompt and the research section", async () => {
  const w = await world(true, true, 3, {}, { researcher: { enabled: true } });
  try {
    await launched(w);
    const old = await w.launcher.spawn("researcher");
    const first = promptOf(w, w.adapter.starts.length - 1);
    const result = await w.launcher.replace(old.agentId);
    assert.equal(result.state, "started");
    const start = w.adapter.starts.at(-1)!;
    assert.ok(start.args.includes("--mcp-config"));
    const replaced = promptOf(w, w.adapter.starts.length - 1);
    assert.match(replaced, /You are the researcher of a Capstan delivery team/);
    assert.match(replaced, /docs\/research\//);
    assert.ok(first.includes("Output contract"));
    assert.ok(replaced.includes("Output contract"));
    await w.launcher.restartPm();
    assert.match(
      promptOf(w, w.adapter.starts.length - 1),
      /Web research \(the Researcher is enabled/,
    );
  } finally {
    w.cleanup();
  }
});

const SETUP: ResolvedWorktree = { setup: "npm ci", setupTimeoutSeconds: 7 };

test("without a worktree configuration spawn never runs setup", async () => {
  const calls: string[] = [];
  const w = await world(
    true,
    true,
    3,
    {},
    {
      runSetup: async (command) => {
        calls.push(command);
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    await w.launcher.spawn("developer");
    assert.deepEqual(calls, []);
  } finally {
    w.cleanup();
  }
});

test("setup runs once in the new worktree after it is created and before the worker starts, for a worker and the architect, not the PM", async () => {
  const calls: Array<[string, string, number]> = [];
  const startsAtSetup: number[] = [];
  const holder: { w?: World } = {};
  const w = await world(
    true,
    true,
    3,
    {},
    {
      architect: { counts: false },
      worktree: SETUP,
      runSetup: async (command, cwd, timeoutMs) => {
        calls.push([command, cwd, timeoutMs]);
        startsAtSetup.push(holder.w!.adapter.starts.length);
        return { status: "ok" };
      },
    },
  );
  holder.w = w;
  try {
    await launched(w);
    assert.deepEqual(calls, []);
    const first = await w.launcher.spawn("developer");
    assert.deepEqual(calls, [["npm ci", first.worktreePath, 7000]]);
    assert.deepEqual(startsAtSetup, [1], "only the PM had started");
    assert.ok(
      w.adapter.calls.findIndex((c) => c.startsWith("worktree:")) <
        w.adapter.calls.findIndex((c) => c.startsWith("place:")) ||
        !w.adapter.calls.some((c) => c.startsWith("place:")),
    );
    await w.launcher.spawn("architect");
    assert.equal(calls.length, 2);
  } finally {
    w.cleanup();
  }
});

test("a failing setup rejects with worktree_setup_failed naming the exit code and output, and removes pane, worktree, branch and agent", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async () => ({
        status: "failed",
        exitCode: 3,
        output: "npm ERR! \u001b[31mboom\u001b[0m\nsecond line",
      }),
    },
  );
  try {
    await launched(w);
    const startsBefore = w.adapter.starts.length;
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worktree_setup_failed" &&
        /developer-1/.test(e.message) &&
        /npm ci/.test(e.message) &&
        /exit code 3/.test(e.message) &&
        /npm ERR! boom second line/.test(e.message),
    );
    assert.equal(w.adapter.starts.length, startsBefore);
    assert.ok(w.adapter.calls.some((c) => c.startsWith("close:")));
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
    assert.equal(
      w.core.listAgents().find((a) => a.agentId === "developer-1")!.state,
      "ended",
    );
    assert.equal(
      w.core.agentPanes(w.owner).some((r) => r.agentId === "developer-1"),
      false,
    );
  } finally {
    w.cleanup();
  }
});

test("a timed-out setup rejects with worktree_setup_failed and the same cleanup", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async () => ({ status: "timeout" }),
    },
  );
  try {
    await launched(w);
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "worktree_setup_failed" &&
        /timed out after 7s/.test(e.message),
    );
    assert.deepEqual(w.git.removed, ["/tmp/work/developer-1"]);
    assert.deepEqual(w.git.deleted, [["capstan/developer-1-g1", SHA]]);
  } finally {
    w.cleanup();
  }
});

test("setup time is not charged to the step budget", async () => {
  let clock = 1_000_000;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async () => {
        clock += 5 * 60_000;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const slow = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: {
        ...config(),
        worktree: SETUP,
      } as CapstanConfig,
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, ".capstan", "state", "control.sock"),
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: { PATH: "/usr/bin:/bin" },
      git: w.git,
      now: () => clock,
      runSetup: async () => {
        clock += 5 * 60_000;
        return { status: "ok" };
      },
    });
    const result = await slow.spawn("developer");
    assert.equal(result.state, "started");
  } finally {
    w.cleanup();
  }
});

test("replace runs setup in the new worktree and reports a setup failure", async () => {
  let fail = false;
  const cwds: string[] = [];
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: SETUP,
      runSetup: async (_command, cwd) => {
        cwds.push(cwd);
        return fail
          ? { status: "failed", exitCode: 1, output: "nope" }
          : { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const old = await w.launcher.spawn("developer");
    const replaced = await w.launcher.replace(old.agentId);
    assert.equal(replaced.state, "started");
    assert.equal(cwds.length, 2);
    if (replaced.state !== "started") return;
    fail = true;
    await assert.rejects(
      w.launcher.replace(replaced.agentId),
      (e: unknown) =>
        e instanceof LauncherError &&
        /worktree_setup_failed|setup/.test(String((e as Error).message)),
    );
  } finally {
    w.cleanup();
  }
});

test("runSetupCommand leaves a succeeding command's effect in the directory", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "capstan-setup-"));
  try {
    const outcome = await runSetupCommand("touch made.txt", dir, 10_000, {
      PATH: "/usr/bin:/bin",
    });
    assert.deepEqual(outcome, { status: "ok" });
    assert.ok(existsSync(path.join(dir, "made.txt")));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("runSetupCommand reports exit code and a capped output tail on failure", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "capstan-setup-"));
  try {
    const outcome = await runSetupCommand(
      "echo oops >&2; exit 4",
      dir,
      10_000,
      { PATH: "/usr/bin:/bin" },
    );
    assert.equal(outcome.status, "failed");
    if (outcome.status === "failed") {
      assert.equal(outcome.exitCode, 4);
      assert.match(outcome.output, /oops/);
    }
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("runSetupCommand kills the whole process group on timeout", async () => {
  const dir = mkdtempSync(path.join(tmpdir(), "capstan-setup-"));
  try {
    const started = Date.now();
    const outcome = await runSetupCommand(
      "sleep 30 & echo $! > child.pid; wait",
      dir,
      1000,
      { PATH: "/usr/bin:/bin" },
    );
    assert.deepEqual(outcome, { status: "timeout" });
    assert.ok(Date.now() - started < 10_000);
    const pid = Number(readFileSync(path.join(dir, "child.pid"), "utf8"));
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.throws(() => process.kill(pid, 0), /ESRCH/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a worktree section loaded from capstan.toml reaches runSetup with its command, cwd and timeout", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-launcher-toml-"));
  const calls: Array<[string, string, number]> = [];
  let w: World | undefined;
  try {
    writeFileSync(
      path.join(directory, CONFIG_FILE_NAME),
      `schema_version = 1

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[worktree]
setup = "make deps"
setup_timeout_seconds = 42
`,
      { mode: 0o600 },
    );
    const loaded = loadCapstanConfig(directory);
    assert.ok(loaded.worktree !== undefined);
    w = await world(
      true,
      true,
      3,
      {},
      {
        worktree: loaded.worktree,
        runSetup: async (command, cwd, timeoutMs) => {
          calls.push([command, cwd, timeoutMs]);
          return { status: "ok" };
        },
      },
    );
    await launched(w);
    const result = await w.launcher.spawn("developer");
    assert.deepEqual(calls, [["make deps", result.worktreePath, 42_000]]);
  } finally {
    w?.cleanup();
    rmSync(directory, { recursive: true, force: true });
  }
});

test("setup runs with the agents' filtered environment: no daemon-only variable, the [env] pass variables present, no agent token", async () => {
  const base = mkdtempSync(path.join(tmpdir(), "capstan-setup-env-"));
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: { setup: "env > setup-env.txt", setupTimeoutSeconds: 10 },
      pass: ["PASSED_VALUE"],
      base: { PASSED_VALUE: "yes", DAEMON_ONLY: "leak" },
    },
  );
  try {
    mkdirSync(path.join(base, "developer-1"));
    w.adapter.worktreeBase = base;
    await launched(w);
    const result = await w.launcher.spawn("developer");
    const text = readFileSync(
      path.join(result.worktreePath, "setup-env.txt"),
      "utf8",
    );
    assert.match(text, /^PASSED_VALUE=yes$/m);
    assert.match(text, /^HOME=\/home\/x$/m);
    assert.doesNotMatch(text, /DAEMON_ONLY|SECRET/);
    assert.doesNotMatch(text, /CAPSTAN_TOKEN/);
  } finally {
    w.cleanup();
    rmSync(base, { recursive: true, force: true });
  }
});

test("the Operator is refused while disabled, takes no worker place unless it counts, and has its own prompt", async () => {
  const off = await world(
    true,
    true,
    3,
    {},
    {
      operator: { counts: false, enabled: false },
    },
  );
  try {
    await launched(off);
    await assert.rejects(
      off.launcher.spawn("operator"),
      (e: unknown) =>
        e instanceof LauncherError && e.code === "operator_disabled",
    );
  } finally {
    off.cleanup();
  }
  const exempt = await world(
    true,
    true,
    1,
    {},
    {
      operator: { counts: false, enabled: true },
    },
  );
  try {
    await launched(exempt);
    const operator = await exempt.launcher.spawn("operator");
    assert.equal(operator.state, "started");
    const dev = await exempt.launcher.spawn("developer");
    assert.equal(dev.state, "started", "the developer still has its place");
    const prompts = exempt.adapter.prompts;
    assert.ok(prompts.some((t) => t.includes("You are the operator")));
    assert.ok(prompts.some((t) => t.includes("the Operator is enabled")));
    assert.ok(
      !prompts.some(
        (t) =>
          t.includes("You are developer (Developer)") &&
          t.includes("cstan op propose"),
      ),
      "an ordinary developer prompt does not mention the operator",
    );
  } finally {
    exempt.cleanup();
  }
  const counted = await world(
    true,
    true,
    1,
    {},
    {
      operator: { counts: true, enabled: true },
    },
  );
  try {
    await launched(counted);
    await counted.launcher.spawn("operator");
    await assert.rejects(
      counted.launcher.spawn("developer"),
      (e: unknown) => e instanceof LauncherError && e.code === "worker_limit",
    );
  } finally {
    counted.cleanup();
  }
});

test("a plain Developer role named operator with no [operator] table spawns with the ordinary developer prompt", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    { operator: { counts: false, enabled: false, table: false } },
  );
  try {
    await launched(w);
    const spawned = await w.launcher.spawn("operator");
    assert.equal(spawned.state, "started");
    const prompt = w.adapter.prompts.at(-1)!;
    assert.ok(prompt.includes("(Developer) on a Capstan delivery team"));
    assert.ok(!prompt.includes("You are the operator"));
    assert.ok(!prompt.includes("cstan op "));
  } finally {
    w.cleanup();
  }
});

test("operatorEnvironment is the filtered setup environment with no CAPSTAN_ variable and no cstan wrapper directory", async () => {
  const w = await world();
  try {
    const wrapperDirectory = path.join(w.root, ".capstan", "bin");
    const launcher = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: config(true, 3, {}, ["KEPT_NAME"]),
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, "control.sock"),
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: {
        PATH: `${wrapperDirectory}:/usr/bin:/bin`,
        HOME: "/home/x",
        CAPSTAN_TOKEN: "agent-token-value",
        CAPSTAN_SOCKET: "/tmp/socket",
        KEPT_NAME: "kept",
        SECRET: "no",
      },
      git: w.git,
    });
    const environment = launcher.operatorEnvironment();
    assert.deepEqual(
      Object.keys(environment).filter((name) => name.startsWith("CAPSTAN_")),
      [],
    );
    assert.equal(environment.PATH, "/usr/bin:/bin");
    assert.equal(environment.KEPT_NAME, "kept");
    assert.equal(environment.HOME, "/home/x");
    assert.equal(environment.SECRET, undefined);
    assert.ok(!JSON.stringify(environment).includes("agent-token-value"));
    // Building it does not create the wrapper, which only agent environments need.
    assert.equal(existsSync(wrapperDirectory), false);
    assert.equal(launcher.inFlightOperations(), 0);
  } finally {
    w.cleanup();
  }
});

test("operatorEnvironment drops PATH when only the wrapper directory was on it", async () => {
  const w = await world();
  try {
    const launcher = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: config(),
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, "control.sock"),
      credential: w.owner,
      baseEnvironment: { PATH: path.join(w.root, ".capstan", "bin") },
      git: w.git,
    });
    assert.equal(launcher.operatorEnvironment().PATH, undefined);
  } finally {
    w.cleanup();
  }
});

test("a failed spawn names the failing step and the real error, whatever the adapter threw", async () => {
  const w = await world();
  try {
    await launched(w);
    w.adapter.startError = new PromptUnrecognized("no bare prompt");
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "spawn_failed" &&
        e.message === "step start: no bare prompt",
    );
    const failed = w.events.find((e) => e.event === "spawn_failed")!;
    assert.deepEqual(failed.details, {
      agentId: "developer-1",
      step: "start",
      error: "step start: no bare prompt",
    });
    w.adapter.startError = new HerdrError("agent_start_failed", "no binary");
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.code === "agent_start_failed" &&
        e.message === "step start: no binary",
    );
    w.adapter.startError = undefined;
    w.adapter.worktreeError = new Error("herdr is gone");
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError &&
        e.message === "step worktree: herdr is gone",
    );
  } finally {
    w.cleanup();
  }
});

test("a worker start is tried again only for an unready prompt or shell, at most three times, and is never retried for another error", async () => {
  const w = await world();
  try {
    await launched(w);
    const base = w.adapter.startAttempts;

    w.adapter.startErrors = [new PromptUnrecognized("first")];
    const second = await w.launcher.spawn("developer");
    assert.equal(second.state, "started");
    assert.equal(w.adapter.startAttempts - base, 2, "succeeds on the 2nd");
    assert.deepEqual(w.sleeps, [1500]);
    assert.equal(w.events.filter((e) => e.event === "start_retry").length, 1);

    const before = w.adapter.startAttempts;
    w.adapter.startErrors = [
      new ShellNotReady("a"),
      new PromptUnrecognized("b"),
      new ShellNotReady("c"),
    ];
    await assert.rejects(
      w.launcher.spawn("developer"),
      (e: unknown) =>
        e instanceof LauncherError && e.message === "step start: c",
    );
    assert.equal(w.adapter.startAttempts - before, 3, "fails after 3");
    assert.deepEqual(w.sleeps, [1500, 1500, 1500]);

    const other = w.adapter.startAttempts;
    w.adapter.startErrors = [new Error("boom")];
    await assert.rejects(w.launcher.spawn("developer"), /step start: boom/);
    assert.equal(
      w.adapter.startAttempts - other,
      1,
      "another error: 1 attempt",
    );
  } finally {
    w.cleanup();
  }
});

test("a retry never runs after the agent started", async () => {
  const w = await world();
  try {
    await launched(w);
    const original = w.adapter.startAgent.bind(w.adapter);
    w.adapter.startAgent = async (input) => {
      await original(input);
      throw new PromptUnrecognized("after the start");
    };
    const before = w.adapter.starts.length;
    await assert.rejects(w.launcher.spawn("developer"), /after the start/);
    assert.equal(w.adapter.starts.length - before, 1);
  } finally {
    w.cleanup();
  }
});

test("a failed start leaves no worktree and no branch in a real repository, and the log has no worktree_kept", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-spawn-git-"));
  const run = (...args: string[]) =>
    spawnSync("git", args, { cwd: root, encoding: "utf8" });
  run("init", "-q");
  run(
    "-c",
    "user.name=t",
    "-c",
    "user.email=t@t",
    "commit",
    "-q",
    "--allow-empty",
    "-m",
    "i",
  );
  const w = await world(true, true, 3, {}, { git: defaultGit(root) });
  try {
    await launched(w);
    const create = w.adapter.createWorktree.bind(w.adapter);
    w.adapter.createWorktree = async (input) => {
      const made = await create(input);
      const checkout = path.join(root, "trees", input.branch.split("/")[1]!);
      const added = run("worktree", "add", "-q", "-b", input.branch, checkout);
      assert.equal(added.status, 0, added.stderr);
      writeFileSync(path.join(checkout, "node_modules.txt"), "dependencies");
      return { ...made, path: checkout };
    };
    w.adapter.startError = new Error("start failed");
    await assert.rejects(w.launcher.spawn("developer"), /step start/);
    const trees = run("worktree", "list", "--porcelain").stdout;
    assert.equal(trees.split("worktree ").length - 1, 1, trees);
    assert.equal(run("branch", "--list", "capstan/*").stdout.trim(), "");
    assert.ok(!w.events.some((e) => e.event === "worktree_kept"));
    const removing = w.events.find((e) => e.event === "worktree_removing")!;
    assert.equal(removing.details.dirty, 1);
  } finally {
    w.cleanup();
    rmSync(root, { recursive: true, force: true });
  }
});

test("release logs how many files a worktree holds before removing it, and git's message when it keeps one", async () => {
  const w = await world();
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.git.dirty = 3;
    await w.launcher.release(first.agentId);
    const removing = w.events.filter((e) => e.event === "worktree_removing");
    assert.deepEqual(removing.at(-1)!.details, {
      agentId: first.agentId,
      dirty: 3,
    });
    const second = await w.launcher.spawn("developer");
    w.git.dirty = 0;
    w.git.removeOk = false;
    w.git.removeStderr = "fatal: cannot remove a locked working tree";
    const kept = await w.launcher.release(second.agentId);
    assert.equal(kept.worktreeRemoved, false);
    assert.deepEqual(
      w.events.filter((e) => e.event === "worktree_removing").at(-1)!.details,
      { agentId: second.agentId, dirty: 0 },
    );
    assert.deepEqual(
      w.events.find((e) => e.event === "worktree_kept")!.details,
      {
        agentId: second.agentId,
        worktreePath: second.worktreePath,
        stderr: "fatal: cannot remove a locked working tree",
      },
    );
  } finally {
    w.cleanup();
  }
});

test("defaultGit removes a Capstan worktree that holds untracked files, and reports git's message for a locked or unknown one", () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-remove-"));
  const run = (...args: string[]) =>
    spawnSync("git", args, { cwd: root, encoding: "utf8" });
  try {
    run("init", "-q");
    run(
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@t",
      "commit",
      "-q",
      "--allow-empty",
      "-m",
      "i",
    );
    const git = defaultGit(root);
    const tree = (branch: string): string => {
      const dir = path.join(root, "trees", branch.replace("/", "-"));
      assert.equal(run("worktree", "add", "-q", "-b", branch, dir).status, 0);
      return dir;
    };

    const dirty = tree("capstan/dev-1-g1");
    writeFileSync(path.join(dirty, "untracked.txt"), "x");
    assert.equal(git.worktreeDirtyCount(dirty), 1);
    assert.equal(git.worktreeRemove(dirty).removed, true);
    assert.equal(existsSync(dirty), false);

    const locked = tree("capstan/dev-2-g1");
    run("worktree", "lock", locked);
    const refused = git.worktreeRemove(locked);
    assert.equal(refused.removed, false);
    assert.match(refused.stderr, /locked/);
    assert.equal(git.worktreeDirtyCount(locked), 0);

    const foreign = path.join(root, "trees", "foreign");
    assert.equal(
      run("worktree", "add", "-q", "-b", "feature/x", foreign).status,
      0,
    );
    writeFileSync(path.join(foreign, "untracked.txt"), "x");
    assert.equal(
      git.worktreeRemove(foreign).removed,
      false,
      "a worktree of another branch is never forced",
    );
    assert.match(git.worktreeRemove(foreign).stderr, /untracked/);

    const unknown = path.join(root, "trees", "nowhere");
    const missing = git.worktreeRemove(unknown);
    assert.equal(missing.removed, false);
    assert.notEqual(missing.stderr, "");
    assert.equal(git.worktreeDirtyCount(unknown), null);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

const TEARDOWN: ResolvedWorktree = {
  setupTimeoutSeconds: 600,
  teardown: "rm -rf cache",
  teardownTimeoutSeconds: 9,
};

test("teardown runs once per cleanup, in the project root, before the worktree is removed", async () => {
  const calls: Array<[string, string, number]> = [];
  const holder: { w?: World } = {};
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async (command, cwd, timeoutMs) => {
        calls.push([command, cwd, timeoutMs]);
        holder.w!.git.order.push("teardown");
        return { status: "ok" };
      },
    },
  );
  holder.w = w;
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    assert.deepEqual(calls, [], "no teardown while the worker lives");
    await w.launcher.release(first.agentId);
    assert.deepEqual(calls, [["rm -rf cache", w.root, 9000]]);
    assert.deepEqual(w.git.order, ["teardown", "remove"]);
    assert.deepEqual(
      eventNames(w).filter((name) => name.startsWith("teardown")),
      ["teardown_started"],
    );
  } finally {
    w.cleanup();
  }
});

test("teardown runs with the agents' filtered environment plus the worktree path and agent id, and with no agent token or socket", async () => {
  const base = mkdtempSync(path.join(tmpdir(), "capstan-teardown-env-"));
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: {
        setupTimeoutSeconds: 600,
        teardown: "env > teardown-env.txt; pwd > teardown-pwd.txt",
        teardownTimeoutSeconds: 10,
      },
      pass: ["PASSED_VALUE"],
      base: { PASSED_VALUE: "yes", DAEMON_ONLY: "leak" },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    const text = readFileSync(path.join(w.root, "teardown-env.txt"), "utf8");
    assert.match(text, /^PASSED_VALUE=yes$/m);
    assert.match(
      text,
      new RegExp(`^CAPSTAN_WORKTREE_PATH=${first.worktreePath}$`, "m"),
    );
    assert.match(text, /^CAPSTAN_AGENT_ID=developer-1$/m);
    assert.doesNotMatch(text, /DAEMON_ONLY|SECRET/);
    assert.doesNotMatch(text, /CAPSTAN_TOKEN|CAPSTAN_SOCKET/);
    const names = text
      .split("\n")
      .map((line) => line.split("=")[0]!)
      .filter((name) => name.startsWith("CAPSTAN_"));
    assert.deepEqual(names.sort(), [
      "CAPSTAN_AGENT_ID",
      "CAPSTAN_WORKTREE_PATH",
    ]);
    assert.equal(
      readFileSync(path.join(w.root, "teardown-pwd.txt"), "utf8").trim(),
      realpathSync(w.root),
    );
  } finally {
    w.cleanup();
    rmSync(base, { recursive: true, force: true });
  }
});

test("a failing teardown and a timed-out teardown are logged and the worktree is still removed", async () => {
  for (const outcome of [
    { status: "failed", exitCode: 7, output: "boom\u001b[0m" },
    { status: "timeout" },
  ] as const) {
    const w = await world(
      true,
      true,
      3,
      {},
      { worktree: TEARDOWN, runTeardown: async () => outcome },
    );
    try {
      await launched(w);
      const first = await w.launcher.spawn("developer");
      const released = await w.launcher.release(first.agentId);
      assert.equal(released.worktreeRemoved, true);
      assert.deepEqual(w.git.removed, [first.worktreePath]);
      const failed = w.events.filter((e) => e.event === "teardown_failed");
      assert.equal(failed.length, 1);
      assert.equal(failed[0]!.details.agentId, "developer-1");
      assert.equal(
        failed[0]!.details.exit,
        outcome.status === "timeout" ? "timeout" : 7,
      );
      if (outcome.status === "failed")
        assert.equal(failed[0]!.details.output, "boom");
    } finally {
      w.cleanup();
    }
  }
});

test("a teardown that throws is logged and the worktree is still removed", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        throw new Error("spawn failed");
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    assert.equal(
      w.events.filter((e) => e.event === "teardown_failed").length,
      1,
    );
  } finally {
    w.cleanup();
  }
});

test("the real teardown runner kills the process group on timeout, logs it and still removes the worktree", async () => {
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: {
        setupTimeoutSeconds: 600,
        teardown: "sleep 30 & echo $! > child.pid; wait",
        teardownTimeoutSeconds: 1,
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    await w.launcher.release(first.agentId);
    assert.deepEqual(w.git.removed, [first.worktreePath]);
    const failed = w.events.filter((e) => e.event === "teardown_failed");
    assert.equal(failed.length, 1);
    assert.equal(failed[0]!.details.exit, "timeout");
    const pid = Number(readFileSync(path.join(w.root, "child.pid"), "utf8"));
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.throws(() => process.kill(pid, 0), /ESRCH/);
  } finally {
    w.cleanup();
  }
});

test("teardown is not run while the pane is still open, and runs on the retry once it closes", async () => {
  let runs = 0;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        runs += 1;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.adapter.closeError = new HerdrError("pane_close_failed", "busy");
    await w.launcher.release(first.agentId);
    assert.equal(runs, 0);
    assert.deepEqual(w.git.removed, []);
    w.adapter.closeError = undefined;
    await w.launcher.spawn("developer");
    assert.equal(runs, 1);
    assert.deepEqual(w.git.removed, [first.worktreePath]);
  } finally {
    w.cleanup();
  }
});

test("teardown runs again when a refused removal is retried", async () => {
  let runs = 0;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        runs += 1;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    const first = await w.launcher.spawn("developer");
    w.git.removeOk = false;
    await w.launcher.release(first.agentId);
    assert.equal(runs, 1);
    w.git.removeOk = true;
    await w.launcher.spawn("developer");
    assert.equal(runs, 2);
  } finally {
    w.cleanup();
  }
});

test("teardown is not run when no worktree path exists", async () => {
  let runs = 0;
  const w = await world(
    true,
    true,
    3,
    {},
    {
      worktree: TEARDOWN,
      runTeardown: async () => {
        runs += 1;
        return { status: "ok" };
      },
    },
  );
  try {
    await launched(w);
    w.adapter.worktreeError = new HerdrError("worktree_failed", "no");
    await assert.rejects(w.launcher.spawn("developer"));
    assert.equal(runs, 0);
    assert.deepEqual(w.git.removed, []);
  } finally {
    w.cleanup();
  }
});

test("a failed setup, a replace and a config without teardown: teardown runs for the first two and never for the last", async () => {
  let runs = 0;
  const runTeardown: TeardownRunner = async () => {
    runs += 1;
    return { status: "ok" };
  };
  const failing = await world(
    true,
    true,
    3,
    {},
    {
      worktree: { ...TEARDOWN, setup: "npm ci" },
      runSetup: async () => ({ status: "timeout" }),
      runTeardown,
    },
  );
  try {
    await launched(failing);
    await assert.rejects(failing.launcher.spawn("developer"));
    assert.equal(runs, 1, "the failed spawn's worktree is torn down");
  } finally {
    failing.cleanup();
  }
  const replaced = await world(
    true,
    true,
    3,
    {},
    { worktree: TEARDOWN, runTeardown },
  );
  try {
    await launched(replaced);
    const old = await replaced.launcher.spawn("developer");
    await replaced.launcher.replace(old.agentId);
    assert.equal(runs, 2, "the replaced agent's worktree is torn down");
  } finally {
    replaced.cleanup();
  }
  runs = 0;
  const plain = await world(
    true,
    true,
    3,
    {},
    { worktree: SETUP, runSetup: async () => ({ status: "ok" }), runTeardown },
  );
  try {
    await launched(plain);
    const first = await plain.launcher.spawn("developer");
    await plain.launcher.release(first.agentId);
    assert.equal(runs, 0);
    assert.deepEqual(
      eventNames(plain).filter((name) => name.startsWith("teardown")),
      [],
    );
    assert.deepEqual(plain.git.removed, [first.worktreePath]);
  } finally {
    plain.cleanup();
  }
});

test("the documented codebase-memory teardown removes exactly the index files of its own worktree", () => {
  const reference = readFileSync("docs/reference/configuration.md", "utf8");
  const match = /^teardown = '(.+)'$/m.exec(reference);
  assert.ok(
    match !== null,
    "the configuration reference has a one-line teardown example",
  );
  const command = match[1]!;
  const home = mkdtempSync(path.join(tmpdir(), "capstan-teardown-home-"));
  try {
    const dir = path.join(home, ".cache", "codebase-memory-mcp");
    mkdirSync(dir, { recursive: true });
    const worktree = "/home/x/.herdr/worktrees/proj/proj-developer-3-g1";
    const own = "home-x-.herdr-worktrees-proj-proj-developer-3-g1";
    const ownFiles = [
      `${own}.db`,
      `${own}.db-shm`,
      `${own}.db-wal`,
      `${own}.db.stage.AbC123`,
      `${own}.db.stage.AbC123.lock`,
    ];
    const others = [
      "home-x-.herdr-worktrees-proj-proj-developer-33-g1.db",
      "home-x-.herdr-worktrees-proj-proj-developer-3-g10.db",
      "home-x-.herdr-worktrees-proj-proj-developer-3-g1-extra.db",
      "home-x-.herdr-worktrees-proj-proj-developer-4-g1.db-wal",
      "home-x-Workspace-proj.db",
      "_config.db",
    ];
    for (const name of [...ownFiles, ...others])
      writeFileSync(path.join(dir, name), "x");
    const run = (id: string) =>
      spawnSync("sh", ["-c", command], {
        env: {
          PATH: "/usr/bin:/bin",
          HOME: home,
          CAPSTAN_WORKTREE_PATH: worktree,
          CAPSTAN_AGENT_ID: id,
        },
        encoding: "utf8",
      });
    assert.equal(run("developer-3").status, 0);
    assert.deepEqual(readdirSync(dir).sort(), [...others].sort());
    assert.equal(run("developer-3").status, 0, "idempotent when run again");
    const empty = spawnSync("sh", ["-c", command], {
      env: { PATH: "/usr/bin:/bin", HOME: home },
    });
    assert.notEqual(empty.status, 0, "no worktree path removes nothing");
    assert.deepEqual(readdirSync(dir).sort(), [...others].sort());
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test("teardown time is not charged to the cleanup budget", async () => {
  let clock = 1_000_000;
  const w = await world();
  try {
    await launched(w);
    const slow = new Launcher({
      core: w.core,
      adapter: w.adapter,
      config: { ...config(), worktree: TEARDOWN } as CapstanConfig,
      projectRoot: w.root,
      cliPath: "/opt/capstan/cli.js",
      socketPath: path.join(w.root, ".capstan", "state", "control.sock"),
      credential: w.owner,
      nodePath: "/usr/bin/node",
      baseEnvironment: { PATH: "/usr/bin:/bin" },
      git: w.git,
      now: () => clock,
      runTeardown: async () => {
        clock += 5 * 60_000;
        return { status: "ok" };
      },
    });
    const first = await slow.spawn("developer");
    const released = await slow.release(first.agentId);
    assert.deepEqual(
      [released.paneClosed, released.worktreeRemoved, released.branchKept],
      [true, true, false],
    );
  } finally {
    w.cleanup();
  }
});

test("interrupt sends exactly one Esc to a working agent, nothing to an idle one, and is refused without prompt_relay", async () => {
  const off = await world();
  try {
    await off.launcher.launchPm();
    const spawned = await off.launcher.spawn("developer");
    await assert.rejects(
      off.launcher.interrupt(spawned.agentId),
      (error: Error) =>
        error instanceof LauncherError && error.code === "not_configured",
    );
    assert.deepEqual(off.adapter.interrupts, []);
  } finally {
    off.cleanup();
  }
  const w = await world(true, true, 3, {}, { promptRelay: true });
  try {
    await w.launcher.launchPm();
    const spawned = await w.launcher.spawn("developer");
    const paneId = w.core
      .agentPanes(w.owner)
      .find((row) => row.agentId === spawned.agentId)!.paneId!;
    assert.equal(await w.launcher.interrupt(spawned.agentId), false);
    assert.deepEqual(w.adapter.interrupts, [], "an idle agent gets no key");
    w.adapter.workingPanes.add(paneId);
    assert.equal(await w.launcher.interrupt(spawned.agentId), true);
    assert.deepEqual(w.adapter.interrupts, ["esc"]);
    assert.ok(
      w.events.some(
        (e) =>
          e.event === "prompt_relay_key" &&
          e.details.key === "esc" &&
          e.details.agentId === spawned.agentId,
      ),
      "the key is logged",
    );
    await assert.rejects(
      w.launcher.interrupt("nobody"),
      (error: Error) =>
        error instanceof LauncherError && error.code === "agent_not_active",
    );
    assert.deepEqual(w.adapter.interrupts, ["esc"]);
  } finally {
    w.cleanup();
  }
});

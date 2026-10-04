/** Shared setup for the launcher tests: the world, the configs and the helpers. */
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  type CapstanConfig,
  type ResolvedWorktree,
} from "../src/config/capstan-config.js";
import { ControllerCore } from "../src/controller/core.js";
import {
  Launcher,
  type GitRunner,
  type SetupRunner,
  type TeardownRunner,
} from "../src/launcher.js";
import { ctx, projectInfo } from "./harness.js";
import { StubAdapter, StubGit } from "./launcher-stubs.js";

export const hashOf = (name: string): string =>
  createHash("sha256").update(name).digest("hex");

export function config(
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
    ledger: { keepMigrationBackups: 3 },
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

export function withArchitect(
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

export function withResearcher(
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

export function withOperator(
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

export interface World {
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

export async function world(
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

export const eventNames = (w: World): string[] => w.events.map((e) => e.event);

export async function launched(w: World): Promise<void> {
  assert.equal((await w.launcher.launchPm()).state, "started");
}

export const PANE = { spawn: "pane" as const };

/** An accepted report by the worker, using the token it was started with. */
export function reportAs(
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

export function promptOf(w: World, startIndex: number): string {
  const args = w.adapter.starts[startIndex]!.args;
  return readFileSync(
    args[args.indexOf("--append-system-prompt-file") + 1]!,
    "utf8",
  );
}

export const SETUP: ResolvedWorktree = {
  setup: "npm ci",
  setupTimeoutSeconds: 7,
};

#!/usr/bin/env node
import { createHash, randomBytes, randomUUID } from "node:crypto";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { ControllerCore, type ControllerStatus } from "./controller/core.js";
import {
  M1BridgeAdapter,
  expectedReceiptPeer,
} from "./controller/m1-bridge.js";
import { FileMutationContextStore } from "./controller/mutation-contexts.js";
import {
  WorkflowScheduler,
  type SchedulerStep,
} from "./controller/scheduler.js";
import {
  validateWorkflowPlan,
  type WorkflowPlan,
} from "./controller/workflow.js";
import type {
  CandidateInput,
  EvidenceInput,
  InitialProject,
  MutationContext,
  ProjectInput,
  Role,
} from "./controller/types.js";
import {
  RoleRuntimeManager,
  type RoleRuntimeContainmentProof,
  type RoleRuntimeSession,
} from "./runtime/role-runtime-manager.js";
import { listenControl, requestControl } from "./control.js";

const CONFIG_NAME = ".capstan/project.json";
const KEY_NAME = ".capstan/operator.key";
const EXIT = Object.freeze({
  ok: 0,
  usage: 2,
  invalid: 3,
  blocked: 4,
  runtime: 5,
});
type Config = {
  schemaVersion: 1;
  projectId: string;
  name: string;
  stateDirectory: string;
  maxSlices: number;
  maxRunMs: number;
  maxDispatches: number;
};
export type CstanRunJsonV1 = {
  schemaVersion: 1;
  state: "complete" | "canceled" | "stopped" | "waiting";
  projectId: string;
  planHash: string;
  baseSha: string;
  roles: readonly {
    role: "PM" | "Developer" | "Verifier" | "Supervisor";
    sessionId: string | null;
  }[];
  scheduler: SchedulerStep;
  blocker?: string;
};
export type CstanStatusJsonV1 = ControllerStatus & {
  schemaVersion: 1;
  ownership: readonly {
    workItemId: string;
    role: string;
    owner: string | null;
  }[];
  blockers: readonly {
    workItemId: string;
    state: string;
    blockers: readonly string[];
  }[];
  limits: Pick<Config, "maxSlices" | "maxRunMs" | "maxDispatches">;
  nextLegalActions: readonly string[];
};
export type CstanInspectJsonV1 = {
  schemaVersion: 1;
  kind: "work_item" | "assignment" | "candidate" | "finding" | "recovery";
  id: string;
  record: Record<string, unknown>;
};
class BlockedError extends Error {}
class InvalidInputError extends Error {}

function fail(message: string, code = EXIT.usage): never {
  process.stderr.write(`${message}\n`);
  process.exitCode = code;
  throw new Error(message);
}

function readJson(file: string): unknown {
  return JSON.parse(fs.readFileSync(file, "utf8")) as unknown;
}

function parseConfig(value: unknown): Config {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new TypeError("project config must be an object");
  const raw = value as Partial<Config>;
  const keys = Object.keys(raw).sort().join(",");
  if (
    keys !==
    "maxDispatches,maxRunMs,maxSlices,name,projectId,schemaVersion,stateDirectory"
  )
    throw new TypeError("project config has unknown or missing fields");
  if (
    raw.schemaVersion !== 1 ||
    typeof raw.projectId !== "string" ||
    !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(raw.projectId)
  )
    throw new TypeError(
      "project config has an invalid schemaVersion or projectId",
    );
  if (
    typeof raw.name !== "string" ||
    raw.name.trim() === "" ||
    raw.name.length > 256
  )
    throw new TypeError("project config name is invalid");
  if (
    typeof raw.stateDirectory !== "string" ||
    !path.isAbsolute(raw.stateDirectory)
  )
    throw new TypeError("project config stateDirectory must be absolute");
  for (const [key, min, max] of [
    ["maxSlices", 2, 8],
    ["maxRunMs", 1_000, 86_400_000],
    ["maxDispatches", 2, 64],
  ] as const) {
    const number = raw[key];
    if (
      typeof number !== "number" ||
      !Number.isSafeInteger(number) ||
      number < min ||
      number > max
    )
      throw new TypeError(
        `project config ${key} must be an integer from ${min} through ${max}`,
      );
  }
  return raw as Config;
}

function workflowWorkItemId(taskId: string, sliceId: string): string {
  return `wf-${createHash("sha256")
    .update(`${taskId}:${sliceId}`)
    .digest("hex")
    .slice(0, 24)}`;
}

function projectInputs(
  config: Config,
  plan: WorkflowPlan,
  baseSha: string,
): readonly ProjectInput[] {
  return [
    {
      kind: "project_config",
      content: {
        schemaVersion: 1,
        projectId: config.projectId,
        name: config.name,
        baseSha,
      },
    },
    {
      kind: "task_brief",
      content: { taskId: plan.taskId, objective: plan.objective },
    },
    { kind: "acceptance_criteria", content: plan.acceptanceCriteria },
    { kind: "policy", content: plan.limits },
    { kind: "plan", content: plan },
  ];
}

function project(
  config: Config,
  credential: string,
  initialInputs: readonly ProjectInput[],
): InitialProject {
  return {
    projectId: config.projectId,
    name: config.name,
    ownerCredential: credential,
    initialInputs,
  };
}

function objectRecord(value: unknown): Record<string, unknown> | undefined {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    return undefined;
  return value as Record<string, unknown>;
}

function context(core: ControllerCore, credential: string): MutationContext {
  const id = randomUUID();
  return {
    credential,
    requestId: id,
    idempotencyKey: id,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

function output(value: unknown, json: boolean): void {
  process.stdout.write(
    json ? `${JSON.stringify(value, null, 2)}\n` : `${render(value)}\n`,
  );
}

function render(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(render).join("\n");
  if (value && typeof value === "object")
    return Object.entries(value)
      .map(([key, item]) => `${key}: ${render(item)}`)
      .join("\n");
  return String(value);
}

function parseOptions(args: string[]): { positional: string[]; json: boolean } {
  const json = args.includes("--json");
  return { positional: args.filter((arg) => arg !== "--json"), json };
}

function loadConfig(cwd: string): { config: Config; credential: string } {
  try {
    const directory = path.join(cwd, ".capstan");
    const configPath = path.join(cwd, CONFIG_NAME);
    const keyPath = path.join(cwd, KEY_NAME);
    const directoryStat = fs.lstatSync(directory);
    if (
      !directoryStat.isDirectory() ||
      directoryStat.isSymbolicLink() ||
      (process.getuid && directoryStat.uid !== process.getuid()) ||
      (directoryStat.mode & 0o077) !== 0
    )
      throw new Error(
        "Capstan project directory must be private and owned by the current user",
      );
    const configStat = fs.lstatSync(configPath);
    const keyStat = fs.lstatSync(keyPath);
    if (
      !configStat.isFile() ||
      configStat.isSymbolicLink() ||
      !keyStat.isFile() ||
      keyStat.isSymbolicLink()
    )
      throw new Error("Capstan config and operator key must be regular files");
    if ((configStat.mode & 0o077) !== 0 || (keyStat.mode & 0o077) !== 0)
      throw new Error(
        "Capstan config and operator key permissions must be 0600",
      );
    if (
      process.getuid &&
      (configStat.uid !== process.getuid() || keyStat.uid !== process.getuid())
    )
      throw new Error("Capstan files must be owned by the current user");
    const config = parseConfig(readJson(configPath));
    if (
      path.resolve(config.stateDirectory) !==
      path.join(cwd, ".capstan", "state")
    )
      throw new Error("Capstan stateDirectory must remain project-local");
    const credential = fs.readFileSync(keyPath, "utf8").trim();
    if (credential.length < 32)
      throw new Error("operator key is missing or invalid");
    return { config, credential };
  } catch (error) {
    if (error instanceof InvalidInputError) throw error;
    throw new InvalidInputError(
      error instanceof Error ? error.message : String(error),
    );
  }
}

async function inspectController(
  config: Config,
  credential: string,
  action: "status" | "inspect",
  id?: string,
): Promise<unknown> {
  const socketPath = path.join(config.stateDirectory, "control.sock");
  try {
    return await requestControl(socketPath, credential, action, id);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (!/ENOENT|ECONNREFUSED|connect/i.test(message)) throw error;
  }
  const databasePath = path.join(config.stateDirectory, "controller.sqlite");
  if (!fs.existsSync(databasePath)) {
    if (action === "inspect")
      throw new Error("controller record does not exist");
    return {
      projectId: config.projectId,
      run: { state: "not_started", stateVersion: 0 },
      stateVersion: 0,
      inputRevision: 0,
      roles: ["PM", "Developer", "Verifier", "Supervisor"].map((role) => ({
        role,
        seatId: null,
        seatState: "not_created",
        actorActive: false,
        sessionState: null,
        assignmentId: null,
      })),
      work: [],
      findings: [],
      evidence: [],
    };
  }
  const core = await ControllerCore.open({
    stateDirectory: config.stateDirectory,
    project: project(config, credential, []),
    workspaceRoot: process.cwd(),
  });
  try {
    return action === "status" ? core.statusSnapshot() : core.inspect(id!);
  } finally {
    core.close();
  }
}

function usage(): never {
  fail(
    "usage: cstan init | cstan run --brief <file> | cstan status [--json] | cstan inspect <id> [--json]",
  );
}

async function runCli(argv: string[]): Promise<number> {
  const [command, ...rest] = argv;
  const cwd = process.cwd();
  if (command === "init") {
    const directory = path.join(cwd, ".capstan");
    try {
      fs.mkdirSync(directory, { mode: 0o700 });
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "EEXIST")
        throw new InvalidInputError(
          "Capstan project directory already exists; refusing to modify existing contents",
        );
      throw error;
    }
    const configPath = path.join(cwd, CONFIG_NAME);
    if (fs.existsSync(configPath))
      throw new Error("Capstan project is already initialized");
    const config: Config = {
      schemaVersion: 1,
      projectId: `p${randomUUID().replaceAll("-", "")}`,
      name: path.basename(cwd),
      stateDirectory: path.join(directory, "state"),
      maxSlices: 4,
      maxRunMs: 3_600_000,
      maxDispatches: 16,
    };
    const credential = randomBytes(32).toString("base64url");
    fs.writeFileSync(path.join(cwd, KEY_NAME), `${credential}\n`, {
      flag: "wx",
      mode: 0o600,
    });
    fs.writeFileSync(configPath, `${JSON.stringify(config, null, 2)}\n`, {
      flag: "wx",
      mode: 0o600,
    });
    fs.mkdirSync(config.stateDirectory, { recursive: true, mode: 0o700 });
    process.stdout.write(
      `Initialized Capstan project ${config.projectId}\nOperator credential: ${path.join(cwd, KEY_NAME)} (0600)\nAdd this path to .gitignore.\n`,
    );
    return EXIT.ok;
  }
  if (command === "run") {
    const briefAt = rest.indexOf("--brief");
    if (briefAt < 0 || !rest[briefAt + 1] || rest.length !== 2) usage();
    const { config, credential } = loadConfig(cwd);
    const briefPath = path.resolve(cwd, rest[briefAt + 1]!);
    let plan: WorkflowPlan;
    try {
      plan = validateWorkflowPlan(readJson(briefPath)).plan;
    } catch (error) {
      if (error instanceof TypeError || error instanceof SyntaxError)
        throw error;
      throw new InvalidInputError(
        error instanceof Error ? error.message : String(error),
      );
    }
    if (
      plan.limits.maxSlices > config.maxSlices ||
      plan.limits.maxRunMs > config.maxRunMs ||
      plan.limits.maxDispatches > config.maxDispatches
    )
      throw new InvalidInputError(
        "brief limits exceed project-local configuration",
      );
    const base = spawnSync(
      "git",
      ["-C", cwd, "rev-parse", "--verify", "HEAD^{commit}"],
      { encoding: "utf8" },
    );
    if (base.status !== 0)
      throw new Error(
        `cannot resolve deterministic project Git base: ${(base.stderr || "git rev-parse failed").trim()}`,
      );
    const baseSha = base.stdout.trim();
    if (!/^[a-f0-9]{40}([a-f0-9]{24})?$/.test(baseSha))
      throw new Error("Git returned an invalid base commit");
    const initialProject = project(
      config,
      credential,
      projectInputs(config, plan, baseSha),
    );
    if (fs.existsSync(path.join(config.stateDirectory, "controller.sqlite")))
      throw new BlockedError(
        "a prior controller run exists; restart reconciliation is required and redispatch is unsafe",
      );
    const providerHost = process.env.M1_PROVIDER_HOST;
    if (!providerHost)
      throw new Error(
        "M1_PROVIDER_HOST is required; no runtime host fallback is permitted",
      );
    const workspaceRoot = path.join(cwd, ".capstan", "workspaces");
    fs.mkdirSync(workspaceRoot, { recursive: true, mode: 0o700 });
    const workspaceRootStat = fs.lstatSync(workspaceRoot);
    if (
      !workspaceRootStat.isDirectory() ||
      workspaceRootStat.isSymbolicLink() ||
      (process.getuid && workspaceRootStat.uid !== process.getuid()) ||
      fs.realpathSync(workspaceRoot) !== path.resolve(workspaceRoot)
    )
      throw new Error(
        "role workspace root must be a project-local directory owned by the current user",
      );
    fs.chmodSync(workspaceRoot, 0o700);
    for (const [name, value] of [
      ["M1_HERDR_BINARY", process.env.M1_HERDR_BINARY],
      ["M1_OMP_BINARY", process.env.M1_OMP_BINARY],
      ["M1_OMP_NATIVE_ADDON", process.env.M1_OMP_NATIVE_ADDON],
      ["M1_NODE_BINARY", process.env.M1_NODE_BINARY],
    ] as const) {
      if (!value || !path.isAbsolute(value))
        throw new Error(
          `${name} must be explicitly configured as an absolute runtime binary path`,
        );
    }
    const manager = new RoleRuntimeManager({
      stateRoot: path.join(config.stateDirectory, "runtime"),
      providerHost,
      ...(process.env.M1_PROVIDER_PORT
        ? { providerPort: Number(process.env.M1_PROVIDER_PORT) }
        : {}),
      herdrBinary: process.env.M1_HERDR_BINARY!,
      ompBinary: process.env.M1_OMP_BINARY!,
      ompNativeAddon: process.env.M1_OMP_NATIVE_ADDON!,
      nodeBinary: process.env.M1_NODE_BINARY!,
    });
    let core: ControllerCore;
    let runtimeSocketRoot: string | undefined;
    try {
      core = await ControllerCore.open({
        stateDirectory: config.stateDirectory,
        project: initialProject,
        workspaceRoot: cwd,
        runtimeWorkspacePath: "/workspace",
      });
    } catch (error) {
      await manager.close();
      throw error;
    }
    try {
      const socketRoot = fs.mkdtempSync(path.join(os.tmpdir(), "cs-"));
      runtimeSocketRoot = socketRoot;
      const socketRootStat = fs.lstatSync(socketRoot);
      if (
        !socketRootStat.isDirectory() ||
        socketRootStat.isSymbolicLink() ||
        (process.getuid && socketRootStat.uid !== process.getuid()) ||
        (socketRootStat.mode & 0o077) !== 0 ||
        fs.realpathSync(socketRoot) !== socketRoot
      )
        throw new Error("temporary runtime socket root is not private");
      const contexts = new FileMutationContextStore(
        path.join(config.stateDirectory, "mutation-contexts.json"),
        credential,
        validateWorkflowPlan(plan).hash,
      );
      const roleNames = ["PM", "Developer", "Verifier", "Supervisor"] as const;
      const seats = {
        PM: { seatId: `pm-${randomUUID()}`, name: "PM", displayName: "PM" },
        Developer: {
          seatId: `developer-${randomUUID()}`,
          name: "Developer",
          displayName: "Developer",
        },
        Verifier: {
          seatId: `verifier-${randomUUID()}`,
          name: "Verifier",
          displayName: "Verifier",
        },
        Supervisor: {
          seatId: `supervisor-${randomUUID()}`,
          name: "Supervisor",
          displayName: "Supervisor",
        },
      };
      const adapters: Record<string, M1BridgeAdapter> = {};
      const peerVerifiers: Record<
        string,
        { current?: (socketFd: number) => boolean }
      > = {};
      const sessions: Record<string, RoleRuntimeSession> = {};
      const allSessions: RoleRuntimeSession[] = [];
      const containedSessions = new Set<string>();
      const proven: RoleRuntimeContainmentProof[] = [];
      const workspaceMetadata: Record<
        string,
        { workspace: string; baseSha: string }
      > = {};
      const candidateWorkspaces: Record<string, string> = {};
      const runtimeAssignments: Record<string, string> = {};
      const runtimeCommands: Record<string, string> = {};
      const uncertainCommandSnapshots = new Map<
        string,
        Awaited<ReturnType<M1BridgeAdapter["inspectUncertainCommand"]>>
      >();
      let dispatches = 0;
      let closeControl: (() => Promise<void>) | undefined;
      let stopping = false;
      let pmWorkItemId = "";
      const signal = () => {
        stopping = true;
      };
      const provisionRuntime = async (
        role: Exclude<Role, "operator" | "controller">,
        seatId: string,
        workItemId: string,
        generation: number,
        assignmentId: string,
        predecessorCandidates: readonly {
          workspace: string;
          commitSha: string;
        }[] = [],
        exactCandidate?: { workspace: string; commitSha: string },
      ): Promise<{
        session: RoleRuntimeSession;
        adapter: M1BridgeAdapter;
        baseSha: string;
        evidenceDirectory?: string;
      }> => {
        const workspace = path.join(
          workspaceRoot,
          seatId,
          workItemId,
          String(generation),
        );
        fs.mkdirSync(path.dirname(workspace), { recursive: true, mode: 0o700 });
        for (const directory of [
          path.join(workspaceRoot, seatId),
          path.join(workspaceRoot, seatId, workItemId),
        ]) {
          const stat = fs.lstatSync(directory);
          if (
            !stat.isDirectory() ||
            stat.isSymbolicLink() ||
            (process.getuid && stat.uid !== process.getuid()) ||
            !fs
              .realpathSync(directory)
              .startsWith(`${workspaceRoot}${path.sep}`)
          )
            throw new Error(
              "assignment workspace parent escaped the project-local workspace root",
            );
          fs.chmodSync(directory, 0o700);
        }
        const cloned = spawnSync(
          "git",
          ["clone", "--no-hardlinks", "--quiet", "--", cwd, workspace],
          { encoding: "utf8" },
        );
        if (cloned.status !== 0)
          throw new Error(
            `cannot create isolated ${role} assignment checkout: ${(cloned.stderr || "git clone failed").trim()}`,
          );
        fs.chmodSync(workspace, 0o700);
        if (role === "Developer") {
          for (const key of ["user.name", "user.email"] as const) {
            const sourceIdentity = spawnSync(
              "git",
              ["-C", cwd, "config", "--get", key],
              { encoding: "utf8" },
            );
            const value =
              sourceIdentity.status === 0 ? sourceIdentity.stdout.trim() : "";
            if (!value)
              throw new Error(
                `Developer candidate commits require project Git ${key}`,
              );
            const isolatedIdentity = spawnSync(
              "git",
              ["-C", workspace, "config", "--local", key, value],
              { encoding: "utf8" },
            );
            if (isolatedIdentity.status !== 0)
              throw new Error(
                `cannot configure ${key} in isolated Developer checkout: ${(isolatedIdentity.stderr || "git config failed").trim()}`,
              );
          }
        }
        if (exactCandidate) {
          const fetched = spawnSync(
            "git",
            [
              "-C",
              workspace,
              "fetch",
              "--quiet",
              "--no-tags",
              exactCandidate.workspace,
              exactCandidate.commitSha,
            ],
            { encoding: "utf8" },
          );
          if (fetched.status !== 0)
            throw new Error(
              `cannot fetch exact candidate ${exactCandidate.commitSha}: ${(fetched.stderr || "git fetch failed").trim()}`,
            );
          const candidateCheckout = spawnSync(
            "git",
            ["-C", workspace, "checkout", "--quiet", "--detach", "FETCH_HEAD"],
            { encoding: "utf8" },
          );
          if (candidateCheckout.status !== 0)
            throw new Error(
              `cannot check out exact candidate ${exactCandidate.commitSha}: ${(candidateCheckout.stderr || "git checkout failed").trim()}`,
            );
        } else {
          const checkedOut = spawnSync(
            "git",
            ["-C", workspace, "checkout", "--quiet", "--detach", baseSha],
            { encoding: "utf8" },
          );
          if (checkedOut.status !== 0)
            throw new Error(
              `cannot check out accepted base ${baseSha}: ${(checkedOut.stderr || "git checkout failed").trim()}`,
            );
          for (const predecessor of predecessorCandidates) {
            const fetched = spawnSync(
              "git",
              [
                "-C",
                workspace,
                "fetch",
                "--quiet",
                "--no-tags",
                predecessor.workspace,
                predecessor.commitSha,
              ],
              { encoding: "utf8" },
            );
            if (fetched.status !== 0)
              throw new Error(
                `cannot fetch accepted predecessor ${predecessor.commitSha}: ${(fetched.stderr || "git fetch failed").trim()}`,
              );
            const merged = spawnSync(
              "git",
              [
                "-C",
                workspace,
                "merge",
                "--quiet",
                "--no-edit",
                "--no-ff",
                "FETCH_HEAD",
              ],
              { encoding: "utf8" },
            );
            if (merged.status !== 0)
              throw new Error(
                `accepted predecessor ${predecessor.commitSha} does not merge cleanly: ${(merged.stderr || "git merge failed").trim()}`,
              );
          }
        }
        const actualBase = spawnSync(
          "git",
          ["-C", workspace, "rev-parse", "HEAD"],
          { encoding: "utf8" },
        );
        if (
          actualBase.status !== 0 ||
          !/^[a-f0-9]{40}([a-f0-9]{24})?$/.test(actualBase.stdout.trim())
        )
          throw new Error("assignment workspace has no valid committed base");
        if (
          exactCandidate &&
          actualBase.stdout.trim().toLowerCase() !==
            exactCandidate.commitSha.toLowerCase()
        )
          throw new Error(
            "Verifier checkout HEAD does not equal its bound candidate commit",
          );
        const homeDirectory = path.join(workspace, ".home");
        fs.mkdirSync(homeDirectory, { recursive: true, mode: 0o700 });
        const homeStat = fs.lstatSync(homeDirectory);
        if (
          !homeStat.isDirectory() ||
          homeStat.isSymbolicLink() ||
          (process.getuid && homeStat.uid !== process.getuid()) ||
          fs.realpathSync(homeDirectory) !== homeDirectory
        )
          throw new Error("isolated checkout home is not a private directory");
        fs.chmodSync(homeDirectory, 0o700);
        const journalMountpoint = path.join(homeDirectory, "bridge.jsonl");
        try {
          fs.writeFileSync(journalMountpoint, "", {
            flag: "wx",
            mode: 0o600,
          });
        } catch (error) {
          if (
            !(error instanceof Error) ||
            !("code" in error) ||
            error.code !== "EEXIST"
          )
            throw error;
        }
        const journalMountStat = fs.lstatSync(journalMountpoint);
        if (
          !journalMountStat.isFile() ||
          journalMountStat.isSymbolicLink() ||
          (process.getuid && journalMountStat.uid !== process.getuid())
        )
          throw new Error("isolated checkout journal mountpoint is not a file");
        fs.chmodSync(journalMountpoint, 0o600);
        const excludePath = path.join(workspace, ".git", "info", "exclude");
        const excludeStat = fs.lstatSync(excludePath);
        if (!excludeStat.isFile() || excludeStat.isSymbolicLink())
          throw new Error(
            "isolated checkout Git exclude is not a regular file",
          );
        const excludeContents = fs.readFileSync(excludePath, "utf8");
        if (!/^\.home\/?$/m.test(excludeContents))
          fs.appendFileSync(
            excludePath,
            `${excludeContents.endsWith("\n") || excludeContents.length === 0 ? "" : "\n"}.home/\n`,
            { mode: 0o600 },
          );
        const bridgeDir = path.join(socketRoot, "bridge", seatId);
        const receiptDir = path.join(socketRoot, "receipt", seatId);
        const journalDir = path.join(config.stateDirectory, "journals");
        for (const directory of [bridgeDir, receiptDir, journalDir]) {
          fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
          fs.chmodSync(directory, 0o700);
        }
        const bridgePath = path.join(bridgeDir, "seat.sock");
        const receiptPath = path.join(receiptDir, "receipt.sock");
        if (!adapters[seatId]) {
          const journalPath = path.join(
            journalDir,
            `${role.toLowerCase()}.jsonl`,
          );
          try {
            fs.writeFileSync(journalPath, "", { flag: "wx", mode: 0o600 });
          } catch (error) {
            if (
              !(error instanceof Error) ||
              !("code" in error) ||
              error.code !== "EEXIST"
            )
              throw error;
            const journal = fs.lstatSync(journalPath);
            if (
              !journal.isFile() ||
              journal.isSymbolicLink() ||
              (journal.mode & 0o077) !== 0 ||
              (process.getuid && journal.uid !== process.getuid())
            )
              throw new Error(
                `role journal is not a private regular file: ${journalPath}`,
              );
          }
          peerVerifiers[seatId] = {};
          const adapter = new M1BridgeAdapter(
            core,
            receiptPath,
            bridgePath,
            {
              authenticate: (socketFd) =>
                peerVerifiers[seatId]?.current?.(socketFd) ?? false,
            },
            { path: journalPath, role },
          );
          await adapter.listen();
          adapters[seatId] = adapter;
        }
        let evidenceDirectory: string | undefined;
        if (role === "Verifier") {
          const evidenceParent = path.join(
            config.stateDirectory,
            "runtime",
            "evidence",
            seatId,
            workItemId,
          );
          fs.mkdirSync(evidenceParent, { recursive: true, mode: 0o700 });
          fs.chmodSync(evidenceParent, 0o700);
          evidenceDirectory = path.join(evidenceParent, String(generation));
          fs.mkdirSync(evidenceDirectory, { mode: 0o700 });
          fs.chmodSync(evidenceDirectory, 0o700);
        }
        const session = await manager.provision(role, seatId, workspace, {
          journalPath: path.join(journalDir, `${role.toLowerCase()}.jsonl`),
          receiptSocketPath: receiptPath,
          bridgeSocketPath: bridgePath,
          ...(evidenceDirectory ? { evidenceDirectory } : {}),
        });
        peerVerifiers[seatId]!.current = expectedReceiptPeer(
          session.helperPath,
          session.expectedOmpHostPid,
        );
        core.createRuntimeSession(context(core, credential), {
          sessionId: session.sessionId,
          seatId,
          assignmentId,
          provider: "m1",
          profile: session.profile,
          workspace,
        });
        core.recordRuntimeIdentity(
          context(core, credential),
          session.sessionId,
          {
            observedPid: session.expectedOmpHostPid,
            containerId: session.containerId,
            endpoint: session.bridgeSocketPath,
          },
        );
        core.transitionRuntimeSession(
          context(core, credential),
          session.sessionId,
          "ready",
        );
        runtimeAssignments[session.sessionId] = assignmentId;
        sessions[seatId] = session;
        allSessions.push(session);
        workspaceMetadata[session.sessionId] = {
          workspace,
          baseSha: actualBase.stdout.trim(),
        };
        return {
          session,
          adapter: adapters[seatId]!,
          baseSha: actualBase.stdout.trim(),
          ...(evidenceDirectory ? { evidenceDirectory } : {}),
        };
      };
      const containRuntime = async (
        session: RoleRuntimeSession,
        assignmentId: string,
      ) => {
        const runtime = core
          .statusSnapshot()
          .roles.find((entry) => entry.seatId === session.seatId);
        if (
          runtime?.sessionState === "ready" ||
          runtime?.sessionState === "working"
        )
          core.transitionRuntimeSession(
            context(core, credential),
            session.sessionId,
            "stopping",
          );
        const proof = await manager!.stopAndContain(
          session.role,
          session.sessionId,
        );
        core.confirmContainment(
          context(core, credential),
          assignmentId,
          JSON.stringify(proof),
        );
        core.transitionRuntimeSession(
          context(core, credential),
          session.sessionId,
          "exited",
        );
        containedSessions.add(session.sessionId);
        return proof;
      };
      const inspectUncertainCommands = async () => {
        for (const session of allSessions) {
          const commandId = runtimeCommands[session.sessionId];
          if (!commandId || uncertainCommandSnapshots.has(session.sessionId))
            continue;
          const state = core.commandState(commandId);
          if (state !== "attempting" && state !== "unknown") continue;
          const adapter = adapters[session.seatId];
          if (!adapter)
            throw new Error(
              `missing bridge adapter for uncertain command ${commandId}`,
            );
          uncertainCommandSnapshots.set(
            session.sessionId,
            await adapter.inspectUncertainCommand(commandId),
          );
        }
      };
      const reconcileInFlightCommands = async () => {
        if (
          ![...uncertainCommandSnapshots.keys()].some((sessionId) => {
            const commandId = runtimeCommands[sessionId];
            return commandId && core.commandState(commandId) === "attempting";
          })
        )
          return;
        const closeServer = closeControl;
        closeControl = undefined;
        await closeServer?.();
        core.close();
        core = await ControllerCore.open({
          stateDirectory: config.stateDirectory,
          project: initialProject,
          workspaceRoot: cwd,
          runtimeWorkspacePath: "/workspace",
        });
      };
      const containPendingSessions = async (cleanupErrors: unknown[]) => {
        try {
          await inspectUncertainCommands();
        } catch (error) {
          cleanupErrors.push(error);
        }
        for (const session of allSessions) {
          if (containedSessions.has(session.sessionId)) continue;
          try {
            const proof = await manager!.stopAndContain(
              session.role,
              session.sessionId,
            );
            if (!proven.some((entry) => entry.sessionId === proof.sessionId))
              proven.push(proof);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        try {
          for (const proof of await manager!.close())
            if (!proven.some((entry) => entry.sessionId === proof.sessionId))
              proven.push(proof);
        } catch (error) {
          cleanupErrors.push(error);
        }
        if (
          allSessions.every(
            (session) =>
              containedSessions.has(session.sessionId) ||
              proven.some((proof) => proof.sessionId === session.sessionId),
          )
        ) {
          try {
            await reconcileInFlightCommands();
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        for (const proof of proven) {
          if (containedSessions.has(proof.sessionId)) continue;
          const session = allSessions.find(
            (entry) => entry.sessionId === proof.sessionId,
          );
          const assignmentId = runtimeAssignments[proof.sessionId];
          if (!session || !assignmentId) {
            cleanupErrors.push(
              new Error(
                `runtime manager returned an unknown containment proof for ${proof.sessionId}`,
              ),
            );
            continue;
          }
          try {
            core.confirmContainment(
              context(core, credential),
              assignmentId,
              JSON.stringify(proof),
              uncertainCommandSnapshots.get(proof.sessionId),
            );
            const runtime = core
              .statusSnapshot()
              .roles.find((entry) => entry.seatId === session.seatId);
            if (runtime?.sessionState === "stopping")
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "exited",
              );
            containedSessions.add(session.sessionId);
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
      };
      const deadlineMs = contexts.startedAtMs + plan.limits.maxRunMs;
      const waitForReport = async (workItemId: string) => {
        while (!stopping && Date.now() < deadlineMs) {
          const report = core.latestCompletedReport(workItemId);
          if (report) return report;
          const delay = Promise.withResolvers<void>();
          setTimeout(delay.resolve, Math.min(250, deadlineMs - Date.now()));
          await delay.promise;
        }
        return undefined;
      };
      process.on("SIGINT", signal);
      process.on("SIGTERM", signal);
      let cleanupComplete = false;
      try {
        closeControl = await listenControl(
          path.join(config.stateDirectory, "control.sock"),
          credential,
          core,
        );
        const scheduler = new WorkflowScheduler({
          plan: validateWorkflowPlan(plan),
          core,
          operatorCredential: credential,
          startedAtMs: contexts.startedAtMs,
          mutationContexts: contexts,
          seats,
          isPlanAccepted: ({ planHash }) => {
            const pmWork = core
              .statusSnapshot()
              .work.find((work) => work.workItemId === pmWorkItemId);
            if (!pmWork || pmWork.state !== "accepted") return false;
            const report = core.latestCompletedReport(pmWorkItemId);
            if (
              !report ||
              report.role !== "PM" ||
              report.authorityState !== "contained" ||
              report.inputRevision !== core.inputRevision
            )
              return false;
            try {
              const response: unknown = JSON.parse(report.reply);
              return (
                response !== null &&
                typeof response === "object" &&
                "planHash" in response &&
                response.planHash === planHash
              );
            } catch (error) {
              if (error instanceof SyntaxError) return false;
              throw error;
            }
          },
          dispatch: async ({ slice, assignment, getMutationContext }) => {
            if (stopping)
              throw new Error("run canceled before Developer dispatch");
            if (dispatches >= plan.limits.maxDispatches)
              throw new Error(
                "maxDispatches exhausted before Developer dispatch",
              );
            dispatches += 1;
            const predecessorCandidates: {
              workspace: string;
              commitSha: string;
            }[] = [];
            for (const dependency of slice.dependsOn) {
              const dependencyId = workflowWorkItemId(plan.taskId, dependency);
              const dependencyWork = objectRecord(
                objectRecord(core.inspect(dependencyId))?.record,
              );
              const candidateId = dependencyWork?.accepted_candidate_id;
              if (typeof candidateId !== "string")
                throw new Error(
                  `accepted predecessor ${dependency} has no core-accepted candidate`,
                );
              const candidateRecord = objectRecord(
                objectRecord(core.inspect(candidateId))?.record,
              );
              const sourceWorkspace = candidateWorkspaces[candidateId];
              if (
                !sourceWorkspace ||
                typeof candidateRecord?.commit_sha !== "string"
              )
                throw new Error(
                  `accepted predecessor candidate ${candidateId} has no verified workspace source`,
                );
              predecessorCandidates.push({
                workspace: sourceWorkspace,
                commitSha: candidateRecord.commit_sha,
              });
            }
            const { session, adapter } = await provisionRuntime(
              "Developer",
              assignment.seatId,
              assignment.workItemId,
              assignment.generation,
              assignment.assignmentId,
              predecessorCandidates,
            );
            runtimeCommands[session.sessionId] = assignment.commandId;
            core.transitionRuntimeSession(
              context(core, credential),
              session.sessionId,
              "working",
            );
            await adapter.dispatchAndStart(
              getMutationContext(),
              assignment.commandId,
            );
          },
        });
        const identities = scheduler.initialize();
        pmWorkItemId = `pm-${randomUUID()}`;
        core.createWorkItem(context(core, credential), {
          workItemId: pmWorkItemId,
          title: `Review and accept plan ${plan.taskId}`,
          description: `Review the already validated plan without changing it. Return JSON {"planHash":"..."} with planHash exactly equal to ${validateWorkflowPlan(plan).hash} to approve it. Any other result leaves the plan unaccepted. The acceptance criteria are: ${plan.acceptanceCriteria.join("; ")}`,
          requiredRole: "PM",
        });
        for (const slice of plan.slices)
          core.addDependency(
            context(core, credential),
            workflowWorkItemId(plan.taskId, slice.id),
            pmWorkItemId,
          );
        core.markReady(context(core, credential), pmWorkItemId);
        const pmAssignment = core.assignWorkItem(
          context(core, credential),
          pmWorkItemId,
          identities.PM.seatId,
        );
        const pmRuntime = await provisionRuntime(
          "PM",
          identities.PM.seatId,
          pmWorkItemId,
          pmAssignment.generation,
          pmAssignment.assignmentId,
        );
        runtimeCommands[pmRuntime.session.sessionId] = pmAssignment.commandId;
        let blocker: string | undefined;
        if (stopping) blocker = "run canceled by signal";
        else {
          if (dispatches >= plan.limits.maxDispatches)
            throw new Error("maxDispatches exhausted before PM dispatch");
          dispatches += 1;
          core.transitionRuntimeSession(
            context(core, credential),
            pmRuntime.session.sessionId,
            "working",
          );
          await pmRuntime.adapter.dispatchAndStart(
            context(core, credential),
            pmAssignment.commandId,
          );
        }
        const pmReport = stopping
          ? undefined
          : await waitForReport(pmWorkItemId);
        if (!pmReport)
          blocker ??= stopping
            ? "run canceled by signal"
            : "PM report did not complete before the bounded run deadline";
        else if (
          pmReport.assignmentId !== pmAssignment.assignmentId ||
          pmReport.role !== "PM" ||
          pmReport.inputRevision !== core.inputRevision
        )
          throw new Error(
            "PM report does not match the active plan-review assignment",
          );
        else {
          await containRuntime(pmRuntime.session, pmAssignment.assignmentId);
          const pmReply = objectRecord(JSON.parse(pmReport.reply));
          const acceptedPlanHash = pmReply?.planHash;
          if (acceptedPlanHash === validateWorkflowPlan(plan).hash)
            core.acceptNonCandidateReport(
              context(core, credential),
              pmWorkItemId,
              pmAssignment.assignmentId,
            );
          else
            blocker = "PM report did not affirm the exact validated plan hash";
        }
        const planWasAccepted = core
          .statusSnapshot()
          .work.some(
            (item) =>
              item.workItemId === pmWorkItemId && item.state === "accepted",
          );
        if (planWasAccepted) {
          const supervisorWorkItemId = `supervisor-${randomUUID()}`;
          let supervisorRuntime:
            | {
                session: RoleRuntimeSession;
                adapter: M1BridgeAdapter;
                baseSha: string;
              }
            | undefined;
          core.createWorkItem(context(core, credential), {
            workItemId: supervisorWorkItemId,
            title: `Supervise bounded run ${plan.taskId}`,
            description:
              'Review the active plan, accepted PM report, completed reports, and current run state without making changes. Return JSON {"observation":"..."} containing the concrete safety, scope, or acceptance issue observed, or state that none was observed.',
            requiredRole: "Supervisor",
          });
          core.addDependency(
            context(core, credential),
            supervisorWorkItemId,
            pmWorkItemId,
          );
          core.markReady(context(core, credential), supervisorWorkItemId);
          const supervisorAssignment = core.assignWorkItem(
            context(core, credential),
            supervisorWorkItemId,
            identities.Supervisor.seatId,
          );
          if (stopping) blocker ??= "run canceled by signal";
          else if (dispatches >= plan.limits.maxDispatches)
            blocker ??= "maxDispatches exhausted before Supervisor dispatch";
          else {
            supervisorRuntime = await provisionRuntime(
              "Supervisor",
              identities.Supervisor.seatId,
              supervisorWorkItemId,
              supervisorAssignment.generation,
              supervisorAssignment.assignmentId,
            );
            runtimeCommands[supervisorRuntime.session.sessionId] =
              supervisorAssignment.commandId;
            dispatches += 1;
            core.transitionRuntimeSession(
              context(core, credential),
              supervisorRuntime.session.sessionId,
              "working",
            );
            await supervisorRuntime.adapter.dispatchAndStart(
              context(core, credential),
              supervisorAssignment.commandId,
            );
            const supervisorReport = await waitForReport(supervisorWorkItemId);
            if (!supervisorReport)
              blocker ??=
                "Supervisor report did not complete before the bounded run deadline";
            else if (
              supervisorReport.assignmentId !==
                supervisorAssignment.assignmentId ||
              supervisorReport.role !== "Supervisor" ||
              supervisorReport.inputRevision !== core.inputRevision
            )
              throw new Error(
                "Supervisor report does not match its active run-supervision assignment",
              );
            else {
              await containRuntime(
                supervisorRuntime.session,
                supervisorAssignment.assignmentId,
              );
              const supervisorReply = objectRecord(
                JSON.parse(supervisorReport.reply),
              );
              if (
                typeof supervisorReply?.observation !== "string" ||
                !supervisorReply.observation.trim()
              )
                throw new Error(
                  "Supervisor returned no structured run observation",
                );
              core.acceptNonCandidateReport(
                context(core, credential),
                supervisorWorkItemId,
                supervisorAssignment.assignmentId,
              );
            }
          }
        } else {
          blocker ??=
            "Supervisor withheld because the PM plan was not accepted";
        }
        let step = await scheduler.step();
        while (
          !stopping &&
          !blocker &&
          step.state !== "complete" &&
          step.state !== "stopped" &&
          Date.now() < deadlineMs
        ) {
          if (step.state !== "dispatched") {
            blocker =
              step.state === "waiting_for_plan_acceptance"
                ? "the PM plan report has not been accepted by ControllerCore"
                : `work ${step.workItemId} is waiting: ${step.reasons.join("; ")}`;
            break;
          }
          const dispatchStep = step;
          const assignment = dispatchStep.assignment;
          const workItemId = assignment.workItemId;
          const slice = plan.slices.find(
            (entry) => entry.id === dispatchStep.sliceId,
          );
          const developerSession = sessions[identities.Developer.seatId];
          const developerReport = await waitForReport(workItemId);
          if (!developerReport) {
            blocker = `Developer report for ${dispatchStep.sliceId} did not complete before the bounded deadline`;
            break;
          }
          if (
            !slice ||
            !developerSession ||
            developerReport.assignmentId !== assignment.assignmentId ||
            developerReport.role !== "Developer" ||
            developerReport.inputRevision !== core.inputRevision
          )
            throw new Error(
              `Developer report for ${dispatchStep.sliceId} does not match its active assignment`,
            );
          const developerMetadata =
            workspaceMetadata[developerSession.sessionId];
          if (!developerMetadata)
            throw new Error(
              "Developer assignment workspace base was not recorded",
            );
          const candidateReply = objectRecord(
            JSON.parse(developerReport.reply),
          );
          const candidateId = candidateReply?.candidateId;
          const commitSha = candidateReply?.commitSha;
          const reportedBaseSha = candidateReply?.baseSha;
          const changedScope = candidateReply?.changedScope;
          const limitations = candidateReply?.limitations;
          if (
            typeof candidateId !== "string" ||
            candidateId.trim().length === 0 ||
            typeof commitSha !== "string" ||
            !/^[a-f0-9]{40}([a-f0-9]{24})?$/i.test(commitSha) ||
            typeof reportedBaseSha !== "string" ||
            reportedBaseSha !== developerMetadata.baseSha ||
            !Array.isArray(changedScope) ||
            changedScope.some((entry) => typeof entry !== "string") ||
            !Array.isArray(limitations) ||
            limitations.some((entry) => typeof entry !== "string")
          )
            throw new Error(
              `Developer report for ${dispatchStep.sliceId} lacks a valid immutable candidate identity`,
            );
          const workspaceHead = spawnSync(
            "git",
            ["-C", developerMetadata.workspace, "rev-parse", "HEAD"],
            { encoding: "utf8" },
          );
          const workspaceStatus = spawnSync(
            "git",
            [
              "-C",
              developerMetadata.workspace,
              "status",
              "--porcelain",
              "--untracked-files=all",
            ],
            { encoding: "utf8" },
          );
          if (
            workspaceHead.status !== 0 ||
            workspaceHead.stdout.trim().toLowerCase() !==
              commitSha.toLowerCase() ||
            workspaceStatus.status !== 0 ||
            workspaceStatus.stdout.trim() !== ""
          )
            throw new Error(
              "Developer candidate commit is not the clean HEAD of its isolated assignment checkout",
            );
          const ancestry = spawnSync(
            "git",
            [
              "-C",
              developerMetadata.workspace,
              "merge-base",
              "--is-ancestor",
              developerMetadata.baseSha,
              commitSha,
            ],
            { encoding: "utf8" },
          );
          if (ancestry.status !== 0)
            throw new Error(
              `Developer candidate is not descended from its recorded base: ${(ancestry.stderr || "base ancestry check failed").trim()}`,
            );
          const diff = spawnSync(
            "git",
            [
              "-C",
              developerMetadata.workspace,
              "diff",
              "--no-renames",
              "--name-only",
              "--diff-filter=ACDMRT",
              `${developerMetadata.baseSha}..${commitSha}`,
            ],
            { encoding: "utf8" },
          );
          if (diff.status !== 0)
            throw new Error(
              `cannot inspect Developer candidate diff: ${(diff.stderr || "git diff failed").trim()}`,
            );
          const actualScope = diff.stdout.trim()
            ? diff.stdout.trim().split("\n").sort()
            : [];
          const reportedScope = [...(changedScope as string[])].sort();
          if (
            actualScope.length === 0 ||
            actualScope.length !== reportedScope.length ||
            actualScope.some((file, index) => file !== reportedScope[index]) ||
            actualScope.some(
              (file) =>
                file.startsWith("/") ||
                file.split(/[\\/]/).includes("..") ||
                !slice.writeScope.some(
                  (scope) =>
                    file === scope ||
                    file.startsWith(`${scope.replace(/\/+$/, "")}/`),
                ),
            )
          )
            throw new Error(
              `Developer candidate ${candidateId} changes files outside its validated scope`,
            );
          const candidate: CandidateInput = {
            candidateId,
            assignmentId: assignment.assignmentId,
            commitSha,
            baseSha: developerMetadata.baseSha,
            changedScope: reportedScope,
            limitations: limitations as string[],
          };
          await containRuntime(developerSession, assignment.assignmentId);
          core.submitCandidate(
            context(core, identities.Developer.credential),
            candidate,
          );
          candidateWorkspaces[candidateId] = developerMetadata.workspace;
          if (stopping) {
            blocker = "run canceled by signal";
            step = await scheduler.step();
            break;
          }
          if (dispatches >= plan.limits.maxDispatches) {
            blocker = "maxDispatches reached before Verifier evidence";
            step = await scheduler.step();
            break;
          }
          const verifierWorkItemId = `verify-${randomUUID()}`;
          core.createWorkItem(context(core, credential), {
            workItemId: verifierWorkItemId,
            title: `Verify ${dispatchStep.sliceId}: ${candidateId}`,
            description: `Independently verify candidate ${candidateId} at exactly commit ${commitSha} against every slice acceptance criterion: ${slice.acceptanceCriteria.join("; ")}. Return JSON with candidateId and evidence, an array containing exactly one {criterion, passed, artifactRef} for every listed criterion. Persist each artifact beneath /evidence; artifactRef must be an absolute /evidence/... path. Do not modify source files.`,
            requiredRole: "Verifier",
            parentWorkItemId: workItemId,
          });
          core.markReady(context(core, credential), verifierWorkItemId);
          const verifierAssignment = core.assignWorkItem(
            context(core, credential),
            verifierWorkItemId,
            identities.Verifier.seatId,
            candidateId,
          );
          const verifierRuntime = await provisionRuntime(
            "Verifier",
            identities.Verifier.seatId,
            verifierWorkItemId,
            verifierAssignment.generation,
            verifierAssignment.assignmentId,
            [],
            { workspace: developerMetadata.workspace, commitSha },
          );
          runtimeCommands[verifierRuntime.session.sessionId] =
            verifierAssignment.commandId;
          dispatches += 1;
          core.transitionRuntimeSession(
            context(core, credential),
            verifierRuntime.session.sessionId,
            "working",
          );
          await verifierRuntime.adapter.dispatchAndStart(
            context(core, credential),
            verifierAssignment.commandId,
          );
          const verifierReport = await waitForReport(verifierWorkItemId);
          if (!verifierReport) {
            blocker = `Verifier report for candidate ${candidateId} did not complete before the bounded deadline`;
            step = await scheduler.step();
            break;
          }
          if (
            verifierReport.assignmentId !== verifierAssignment.assignmentId ||
            verifierReport.role !== "Verifier" ||
            verifierReport.inputRevision !== core.inputRevision
          )
            throw new Error(
              "Verifier report does not match its candidate-bound assignment",
            );
          const verifierReply = objectRecord(JSON.parse(verifierReport.reply));
          if (
            verifierReply?.candidateId !== candidateId ||
            !Array.isArray(verifierReply.evidence) ||
            verifierReply.evidence.length !== slice.acceptanceCriteria.length
          )
            throw new Error(
              "Verifier report does not provide one evidence result for every slice criterion",
            );
          const seenCriteria = new Set<string>();
          const evidence: EvidenceInput[] = [];
          const evidenceDirectory = verifierRuntime.evidenceDirectory;
          if (!evidenceDirectory)
            throw new Error(
              "Verifier runtime has no dedicated writable evidence mount",
            );
          for (const rawEvidence of verifierReply.evidence) {
            const entry = objectRecord(rawEvidence);
            const criterion = entry?.criterion;
            const artifactRef = entry?.artifactRef;
            if (
              typeof criterion !== "string" ||
              !slice.acceptanceCriteria.includes(criterion) ||
              seenCriteria.has(criterion) ||
              typeof entry?.passed !== "boolean" ||
              typeof artifactRef !== "string" ||
              !artifactRef.startsWith("/evidence/")
            )
              throw new Error(
                "Verifier evidence does not match the exact candidate and criterion set",
              );
            seenCriteria.add(criterion);
            const evidenceRelative = artifactRef.slice("/evidence/".length);
            if (
              !evidenceRelative ||
              path.isAbsolute(evidenceRelative) ||
              evidenceRelative.split(/[\\/]/).includes("..")
            )
              throw new Error(
                "Verifier artifactRef escapes its dedicated evidence mount",
              );
            const artifactPath = path.resolve(
              evidenceDirectory,
              evidenceRelative,
            );
            const artifactRelative = path.relative(
              evidenceDirectory,
              artifactPath,
            );
            const artifactStat = fs.lstatSync(artifactPath);
            const artifactRealPath = fs.realpathSync(artifactPath);
            const evidenceRealPath = fs.realpathSync(evidenceDirectory);
            if (
              !artifactRelative ||
              artifactRelative.startsWith("..") ||
              path.isAbsolute(artifactRelative) ||
              !artifactStat.isFile() ||
              artifactStat.isSymbolicLink() ||
              artifactStat.size === 0 ||
              !artifactRealPath.startsWith(`${evidenceRealPath}${path.sep}`)
            )
              throw new Error(
                "Verifier artifact is not a non-empty regular file in its private evidence directory",
              );
            evidence.push({
              evidenceId: randomUUID(),
              candidateId,
              criterion,
              passed: entry.passed,
              artifactRef: artifactRealPath,
            });
          }
          if (
            slice.acceptanceCriteria.some(
              (criterion) => !seenCriteria.has(criterion),
            )
          )
            throw new Error("Verifier omitted a slice acceptance criterion");
          await containRuntime(
            verifierRuntime.session,
            verifierAssignment.assignmentId,
          );
          for (const item of evidence)
            core.recordEvidence(
              context(core, identities.Verifier.credential),
              verifierAssignment.assignmentId,
              item,
            );
          const failed = evidence.find((item) => !item.passed);
          if (failed) {
            blocker = `Verifier rejected candidate ${candidateId} for criterion ${failed.criterion}`;
            step = await scheduler.step();
            break;
          }
          core.acceptCandidate(
            context(core, credential),
            workItemId,
            candidateId,
          );
          step = await scheduler.step();
        }
        if (stopping) blocker = "run canceled by signal";
        else if (!blocker && step.state === "complete") blocker = undefined;
        else if (!blocker && step.state === "stopped") blocker = step.reason;
        else if (!blocker && Date.now() >= deadlineMs)
          blocker = "maxRunMs exceeded";
        process.off("SIGINT", signal);
        process.off("SIGTERM", signal);
        const cleanupErrors: unknown[] = [];
        for (const session of allSessions) {
          if (containedSessions.has(session.sessionId)) continue;
          const runtime = core
            .statusSnapshot()
            .roles.find((entry) => entry.seatId === session.seatId);
          if (
            runtime?.sessionState === "ready" ||
            runtime?.sessionState === "working"
          ) {
            try {
              core.transitionRuntimeSession(
                context(core, credential),
                session.sessionId,
                "stopping",
              );
            } catch (error) {
              cleanupErrors.push(error);
            }
          }
        }
        await containPendingSessions(cleanupErrors);
        const closeServer = closeControl;
        closeControl = undefined;
        try {
          await closeServer?.();
        } catch (error) {
          cleanupErrors.push(error);
        }
        for (const adapter of Object.values(adapters)) {
          try {
            await adapter.close();
          } catch (error) {
            cleanupErrors.push(error);
          }
        }
        if (cleanupErrors.length)
          throw new AggregateError(
            cleanupErrors,
            `cstan cleanup could not prove complete containment and closure: ${cleanupErrors.map(String).join("; ")}`,
          );
        if (step.state === "complete")
          core.transitionRun(context(core, credential), "completed");
        else if (stopping)
          core.transitionRun(context(core, credential), "canceled");
        else core.transitionRun(context(core, credential), "failed");
        const runOutput: CstanRunJsonV1 = {
          schemaVersion: 1,
          state:
            step.state === "complete"
              ? "complete"
              : stopping
                ? "canceled"
                : step.state === "stopped"
                  ? "stopped"
                  : "waiting",
          projectId: core.projectId,
          planHash: validateWorkflowPlan(plan).hash,
          baseSha,
          roles: roleNames.map((role) => ({
            role,
            sessionId: sessions[identities[role].seatId]?.sessionId ?? null,
          })),
          scheduler: step,
          ...(blocker ? { blocker } : {}),
        };
        output(runOutput, true);
        cleanupComplete = true;
        return step.state === "complete" ? EXIT.ok : EXIT.blocked;
      } finally {
        const cleanupErrors: unknown[] = [];
        if (!cleanupComplete) {
          for (const session of allSessions) {
            if (containedSessions.has(session.sessionId)) continue;
            const runtime = core
              .statusSnapshot()
              .roles.find((entry) => entry.seatId === session.seatId);
            if (
              runtime?.sessionState === "ready" ||
              runtime?.sessionState === "working"
            ) {
              try {
                core.transitionRuntimeSession(
                  context(core, credential),
                  session.sessionId,
                  "stopping",
                );
              } catch (error) {
                cleanupErrors.push(error);
              }
            }
          }
          await containPendingSessions(cleanupErrors);
          const closeServer = closeControl;
          closeControl = undefined;
          try {
            await closeServer?.();
          } catch (error) {
            cleanupErrors.push(error);
          }
          for (const adapter of Object.values(adapters)) {
            try {
              await adapter.close();
            } catch (error) {
              cleanupErrors.push(error);
            }
          }
          if (cleanupErrors.length)
            process.stderr.write(
              `cstan cleanup warning: ${cleanupErrors.map(String).join("; ")}\n`,
            );
        }
        if (cleanupErrors.length === 0) {
          try {
            if (core.statusSnapshot().run.state === "active")
              core.transitionRun(context(core, credential), "failed");
          } catch (error) {
            process.stderr.write(
              `cstan cleanup warning: could not mark aborted run failed: ${String(error)}\n`,
            );
          }
        }

        process.off("SIGINT", signal);
        process.off("SIGTERM", signal);
      }
    } finally {
      try {
        await manager.close();
        if (runtimeSocketRoot)
          fs.rmSync(runtimeSocketRoot, { recursive: true, force: true });
      } finally {
        core.close();
      }
    }
  }
  if (command === "status") {
    const { config, credential } = loadConfig(cwd);
    const parsed = parseOptions(rest);
    if (parsed.positional.length !== 0) usage();
    const snapshot = (await inspectController(
      config,
      credential,
      "status",
    )) as ControllerStatus;
    const blockers = snapshot.work
      .filter((item) => item.state !== "accepted")
      .map((item) => ({
        workItemId: item.workItemId,
        state: item.state,
        blockers: item.blockers,
      }));
    const nextLegalActions = [
      ...new Set(snapshot.work.flatMap((item) => item.nextLegalActions)),
    ];
    const statusOutput: CstanStatusJsonV1 = {
      schemaVersion: 1,
      ...snapshot,
      ownership: snapshot.work.map((item) => ({
        workItemId: item.workItemId,
        role: item.role,
        owner: item.owner,
      })),
      blockers,
      limits: {
        maxSlices: config.maxSlices,
        maxRunMs: config.maxRunMs,
        maxDispatches: config.maxDispatches,
      },
      nextLegalActions: nextLegalActions.length
        ? nextLegalActions
        : snapshot.run.state === "not_started"
          ? ["cstan run --brief <file>"]
          : [],
    };
    output(statusOutput, parsed.json);
    return EXIT.ok;
  }
  if (command === "inspect") {
    const parsed = parseOptions(rest);
    const id = parsed.positional[0];
    if (!id || parsed.positional.length !== 1) usage();
    const { config, credential } = loadConfig(cwd);
    const inspected = await inspectController(
      config,
      credential,
      "inspect",
      id,
    );
    if (parsed.json) {
      const value = objectRecord(inspected);
      const kinds = [
        "work_item",
        "assignment",
        "candidate",
        "finding",
        "recovery",
      ] as const;
      if (
        !value ||
        typeof value.kind !== "string" ||
        !kinds.includes(value.kind as CstanInspectJsonV1["kind"]) ||
        typeof value.id !== "string" ||
        !objectRecord(value.record)
      )
        throw new Error(
          "Controller returned an inspect record outside the cstan inspect JSON contract",
        );
      const inspectOutput: CstanInspectJsonV1 = {
        schemaVersion: 1,
        kind: value.kind as CstanInspectJsonV1["kind"],
        id: value.id,
        record: value.record as Record<string, unknown>,
      };
      output(inspectOutput, true);
    } else output(inspected, false);
    return EXIT.ok;
  }
  usage();
}

const isMain =
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  runCli(process.argv.slice(2))
    .then((code) => {
      process.exitCode = code;
    })
    .catch((error: unknown) => {
      if (process.exitCode === undefined || process.exitCode === 0)
        process.exitCode =
          error instanceof BlockedError
            ? EXIT.blocked
            : error instanceof InvalidInputError ||
                error instanceof TypeError ||
                error instanceof SyntaxError
              ? EXIT.invalid
              : EXIT.runtime;
      process.stderr.write(
        `cstan: ${error instanceof Error ? error.message : String(error)}\n`,
      );
    });
}

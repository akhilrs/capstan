/**
 * Black-box parity harness: one real scratch daemon per client, the same scenario run once with the Node CLI and once with
 * the Rust front end, the same bytes expected back.
 *
 * Normalisation (the one rule, applied by `Normaliser`): ids, timestamps, pids and scratch paths are replaced (and the ages that a timestamp turns into, `PAUSED (3s)` and `oldest 2 min`), each by a
 * pattern for that kind of field, never a whole line. A replaced value keeps its kind in the placeholder (`<TS:z>` for a
 * UTC timestamp, `<TS:local>` for one without a zone, `<UUID#2>` for the second distinct uuid of the scenario, `<PID>`,
 * `<ROOT>`, `<TOKEN:developer>`), so a different kind of value, a different place in the text, or an id that stops
 * matching an earlier one still shows as a difference. Everything else (words, spacing, counts, exit codes) is compared
 * exactly.
 *
 * The suite needs the Rust front end (CSTAN_FRONT_BIN, else rust/target/release/cstan from `npm run check:dash`) and
 * `npm run build` for dist/src/cli.js. Without the binary it FAILS with "build the Rust front end: npm run check:dash".
 * A machine without cargo sets both CSTAN_SKIP_DASH_CHECK=1 (check:dash then builds nothing) and CSTAN_SKIP_FRONT_PARITY=1
 * (this suite is then skipped, visibly); neither is ever implied by the other.
 */
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { frontEndBinary } from "./cli-transcript-export.js";
import { tempDir, removeTempDir } from "./tmp.js";
import { ControllerCore } from "../src/controller/core.js";
import { openSqlite } from "../src/controller/sqlite.js";
import type {
  InitialProject,
  MutationContext,
} from "../src/controller/types.js";

const repositoryRoot = path.resolve(import.meta.dirname, "..", "..");
export const NODE_CLI = path.join(repositoryRoot, "dist", "src", "cli.js");
export const FIXTURE_DIRECTORY = path.join(
  repositoryRoot,
  "test",
  "fixtures",
  "cli-parity",
);
export const BUILD_HINT = "build the Rust front end: npm run check:dash";

export type Client = "node" | "rust";
export type Identity = "operator" | "pm" | "developer" | "none";

/** One command of a scenario. */
export interface Step {
  readonly as: Identity;
  readonly argv: readonly string[];
  /** Added to the environment of this step; `$ROOT` is the scratch project, `$SOCKET` its socket. */
  readonly env?: Readonly<Record<string, string>>;
  /** Not compared: always run by the Node CLI, to put the ledger in the state the scenario starts from. */
  readonly setup?: boolean;
  /** Run by the Node CLI once the developer's wait has begun in the daemon, while this step is still running (a message arriving for a wait). */
  readonly during?: {
    readonly as: Identity;
    readonly argv: readonly string[];
  };
  /** The step runs from this directory below the scratch project (default: the project). */
  readonly cwd?: string;
  /** What the Node CLI must have done, so two clients that fail the same wrong way cannot pass: its exit code, and text its output holds. */
  readonly expect?: {
    readonly exit?: number;
    readonly contains?: readonly string[];
  };
}

export interface Scenario {
  readonly name: string;
  /** Appended to capstan.toml before the daemon starts (a table the starter file keeps commented out). */
  readonly toml?: string;
  /** Directories made below the scratch project before the daemon starts (another project's `.capstan`, say). */
  readonly dirs?: readonly string[];
  readonly steps: readonly Step[];
}

export interface Observed {
  readonly argv: readonly string[];
  readonly stdout: string;
  readonly stderr: string;
  readonly exit: number | null;
}

interface Member {
  readonly agentId: string;
  readonly actorId: string;
  readonly token: string;
}

const STEP_LIMIT_MS = 60_000;

/** The front end binary, or the failure that names how to build it. null: skipped on purpose (CSTAN_SKIP_FRONT_PARITY=1). */
export function locateFrontEnd(): string | null {
  if (process.env.CSTAN_SKIP_FRONT_PARITY === "1") return null;
  const binary = frontEndBinary();
  if (!existsSync(binary))
    throw new Error(
      `${BUILD_HINT} (no Rust front end at ${binary}; CSTAN_SKIP_FRONT_PARITY=1 skips this suite on a machine without cargo, together with CSTAN_SKIP_DASH_CHECK=1 for npm run check)`,
    );
  if (!existsSync(NODE_CLI))
    throw new Error(`build the Node CLI: npm run build (no ${NODE_CLI})`);
  return binary;
}

/** The environment a client runs in: this process's, without any agent identity, plus what a scenario sets. */
function baseEnvironment(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = { ...process.env };
  for (const name of [
    "CAPSTAN_TOKEN",
    "CAPSTAN_SOCKET",
    "CAPSTAN_AGENT_ID",
    "CAPSTAN_ALLOW_FOREIGN_SOCKET",
    "CSTAN_NODE_CLI",
    "CSTAN_NODE",
    "CSTAN_FRONT_END",
  ])
    delete env[name];
  env.CAPSTAN_LAUNCH = "off";
  return env;
}

const ROLE_HASH = { pm: "a".repeat(64), developer: "b".repeat(64) };
const PROJECT_INPUT_KINDS = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

/** A fresh scratch project with a running daemon, a PM and a developer, in the same ledger state every time. */
export class ParityProject {
  readonly root: string;
  readonly socket: string;
  readonly members: Readonly<Record<"pm" | "developer", Member>>;
  readonly #frontEnd: string;
  readonly #ids = new Map<string, number>();

  private constructor(
    root: string,
    members: Record<"pm" | "developer", Member>,
    frontEnd: string,
  ) {
    this.root = root;
    this.socket = path.join(root, ".capstan", "state", "control.sock");
    this.members = members;
    this.#frontEnd = frontEnd;
  }

  /** `cstan init`, two agents registered in the ledger, a short wait timeout, `cstan start`. */
  static async create(
    frontEnd: string,
    extraToml = "",
    dirs: readonly string[] = [],
  ): Promise<ParityProject> {
    const root = realpathSync(tempDir("cph-"));
    const git = (...args: string[]): void => {
      const result = spawnSync("git", ["-C", root, ...args], {
        encoding: "utf8",
      });
      if (result.status !== 0)
        throw new Error(`git ${args.join(" ")}: ${result.stderr}`);
    };
    git("init", "--quiet");
    git(
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@example.com",
      "commit",
      "--quiet",
      "--allow-empty",
      "-m",
      "chore: initial commit",
    );
    const node = (...argv: string[]): void => {
      const result = spawnSync(process.execPath, [NODE_CLI, ...argv], {
        cwd: root,
        encoding: "utf8",
        env: baseEnvironment(),
      });
      if (result.status !== 0)
        throw new Error(`cstan ${argv.join(" ")}: ${result.stderr}`);
    };
    node("init");
    // A short daemon-side wait: a wait that is not answered ends after eight seconds, long enough for a message sent a moment later to reach it.
    const tomlPath = path.join(root, "capstan.toml");
    const toml = readFileSync(tomlPath, "utf8");
    const patched = toml.replace(
      /\[hosts\.claude\]\nkind = "claude"\n/,
      '[hosts.claude]\nkind = "claude"\nshell_command_timeout_seconds = 20\nwait_timeout_seconds = 8\n',
    );
    if (patched === toml)
      throw new Error("capstan.toml has no [hosts.claude] block to patch");
    writeFileSync(tomlPath, `${patched}\n${extraToml}\n`);
    for (const dir of dirs)
      mkdirSync(path.join(root, dir), { recursive: true });
    const members = await seed(root);
    node("start");
    return new ParityProject(root, members, frontEnd);
  }

  /** The environment of a step for an identity. */
  #environment(
    identity: Identity,
    extra: Readonly<Record<string, string>> | undefined,
  ): NodeJS.ProcessEnv {
    const env = baseEnvironment();
    if (identity === "pm" || identity === "developer") {
      const member = this.members[identity];
      env.CAPSTAN_TOKEN = member.token;
      env.CAPSTAN_SOCKET = this.socket;
      env.CAPSTAN_AGENT_ID = member.agentId;
    }
    for (const [name, value] of Object.entries(extra ?? {}))
      env[name] = value
        .replaceAll("$ROOT", this.root)
        .replaceAll("$SOCKET", this.socket);
    return env;
  }

  /** Runs `cstan <argv>` as a client; the Rust front end is told where the Node CLI is, as an agent's wrapper does. */
  run(
    client: Client,
    step: Pick<Step, "as" | "argv" | "env" | "cwd">,
  ): Promise<Observed> {
    const env = this.#environment(step.as, step.env);
    const [command, args] =
      client === "node" ? [process.execPath, [NODE_CLI]] : [this.#frontEnd, []];
    if (client === "rust") {
      env.CSTAN_NODE_CLI = NODE_CLI;
      env.CSTAN_NODE = process.execPath;
    }
    const argv = [...args, ...step.argv];
    return new Promise((resolve, reject) => {
      const child = spawn(command, argv, {
        cwd:
          step.cwd === undefined ? this.root : path.join(this.root, step.cwd),
        env,
        stdio: ["ignore", "pipe", "pipe"],
      });
      let stdout = "";
      let stderr = "";
      child.stdout.setEncoding("utf8").on("data", (c: string) => {
        stdout += c;
      });
      child.stderr.setEncoding("utf8").on("data", (c: string) => {
        stderr += c;
      });
      const limit = setTimeout(() => {
        child.kill("SIGKILL");
        reject(
          new Error(
            `${client} cstan ${step.argv.join(" ")} ran over its limit`,
          ),
        );
      }, STEP_LIMIT_MS);
      child.once("error", (error) => {
        clearTimeout(limit);
        reject(error);
      });
      child.once("close", (exit) => {
        clearTimeout(limit);
        resolve({ argv: step.argv, stdout, stderr, exit });
      });
    });
  }

  /** Messages and refusals as the ledger holds them, with ids, timestamps and agents in the same normal form as output. */
  ledger(normaliser: Normaliser): unknown {
    const database = openSqlite(
      path.join(this.root, ".capstan", "state", "controller.sqlite"),
      { readOnly: true },
    );
    try {
      const labels = new Map<string, string>(
        (
          [
            ["pm", this.members.pm],
            ["developer", this.members.developer],
          ] as const
        ).map(([label, member]) => [member.actorId, label]),
      );
      const actor = (id: string): string => labels.get(id) ?? "other";
      const messages = (
        database
          .prepare(
            "SELECT message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id, body, body_hash, state, state_version, deferral_count, send_attempts, state_reason, queued_at, sent_at, acked_at, notified_at FROM messages ORDER BY sequence",
          )
          .all() as Record<string, string | number | null>[]
      ).map((row) => ({
        id: normaliser.id(String(row.message_id)),
        sequence: row.sequence,
        recipient: row.recipient_agent_id,
        generation: row.recipient_generation,
        sender: actor(String(row.sender_actor_id)),
        body: normaliser.text(String(row.body)),
        bodyHash: row.body_hash,
        state: row.state,
        stateVersion: row.state_version,
        deferrals: row.deferral_count,
        attempts: row.send_attempts,
        reason: row.state_reason,
        // Whether a time is recorded is compared; which time is not.
        queued: row.queued_at !== null,
        sent: row.sent_at !== null,
        acked: row.acked_at !== null,
        notified: row.notified_at !== null,
      }));
      const rejections = (
        database
          .prepare(
            "SELECT sequence, action, code, from_state, attempted_state, actor_id, reason FROM message_rejections ORDER BY sequence",
          )
          .all() as Record<string, string | number | null>[]
      ).map((row) => ({
        sequence: row.sequence,
        action: row.action,
        code: row.code,
        from: row.from_state,
        attempted: row.attempted_state,
        actor: actor(String(row.actor_id)),
        reason: normaliser.text(String(row.reason)),
      }));
      return { messages, rejections };
    } finally {
      database.close();
    }
  }

  /** The id of the nth message the ledger holds (1 is the first), for a step that names one. */
  messageId(sequence: number): string {
    const database = openSqlite(
      path.join(this.root, ".capstan", "state", "controller.sqlite"),
      { readOnly: true },
    );
    try {
      const row = database
        .prepare("SELECT message_id FROM messages WHERE sequence = ?")
        .get(sequence) as { message_id: string } | undefined;
      if (row === undefined) throw new Error(`no message ${sequence} yet`);
      return row.message_id;
    } finally {
      database.close();
    }
  }

  /** Resolves once the daemon has recorded a wait of the agent, so a message sent next reaches a waiting agent. */
  async waitBegun(member: "pm" | "developer"): Promise<void> {
    const deadline = Date.now() + STEP_LIMIT_MS / 2;
    for (;;) {
      const database = openSqlite(
        path.join(this.root, ".capstan", "state", "controller.sqlite"),
        { readOnly: true },
      );
      try {
        const row = database
          .prepare("SELECT 1 AS begun FROM agent_waits WHERE agent_id = ?")
          .get(this.members[member].agentId);
        if (row !== undefined) return;
      } finally {
        database.close();
      }
      if (Date.now() > deadline) throw new Error("no wait was opened");
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }

  /** `$MESSAGE<n>` in an argument becomes the id of the nth message. */
  resolve(argv: readonly string[]): string[] {
    return argv.map((arg) =>
      arg.replace(/\$MESSAGE(\d+)/g, (_, n: string) =>
        this.messageId(Number(n)),
      ),
    );
  }

  /** Stops the daemon (whatever started it) and removes the scratch project. */
  close(): void {
    spawnSync(process.execPath, [NODE_CLI, "stop"], {
      cwd: this.root,
      env: baseEnvironment(),
      timeout: 30_000,
    });
    removeTempDir(this.root);
  }

  normaliser(): Normaliser {
    return new Normaliser(this.root, this.members, this.#ids);
  }
}

function mutation(core: ControllerCore, credential: string): MutationContext {
  const id = crypto.randomUUID();
  return {
    credential,
    requestId: `req-${id}`,
    idempotencyKey: `idem-${id}`,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

/** Registers a PM and a developer in the project's ledger while no daemon owns it; returns their tokens. */
async function seed(root: string): Promise<Record<"pm" | "developer", Member>> {
  const config = JSON.parse(
    readFileSync(path.join(root, ".capstan", "project.json"), "utf8"),
  ) as { projectId: string; name: string; stateDirectory: string };
  const owner = readFileSync(
    path.join(root, ".capstan", "operator.key"),
    "utf8",
  ).trim();
  const project: InitialProject = {
    projectId: config.projectId,
    name: config.name,
    ownerCredential: owner,
    initialInputs: PROJECT_INPUT_KINDS.map((kind) => ({
      kind,
      content:
        kind === "acceptance_criteria" ? ["criterion"] : { kind, revision: 1 },
    })),
  };
  const core = await ControllerCore.open({
    stateDirectory: config.stateDirectory,
    project,
  });
  try {
    core.syncRoleDefinitions(mutation(core, owner), [
      { name: "pm", kind: "PM", host: "claude", configHash: ROLE_HASH.pm },
      {
        name: "developer",
        kind: "Developer",
        host: "claude",
        configHash: ROLE_HASH.developer,
      },
    ]);
    const member = (
      name: "pm" | "developer",
      kind: "PM" | "Developer",
    ): Member => {
      const seatId = `${name}-seat`;
      core.createSeat(mutation(core, owner), { seatId, name, role: kind });
      const actor = core.createActor(mutation(core, owner), {
        displayName: name,
        role: kind,
        seatId,
      });
      const agentId = `${name}-1`;
      core.registerAgent(mutation(core, owner), {
        agentId,
        roleName: name,
        seatId,
        actorId: actor.actorId,
      });
      return { agentId, actorId: actor.actorId, token: actor.credential };
    };
    return {
      pm: member("pm", "PM"),
      developer: member("developer", "Developer"),
    };
  } finally {
    core.close();
  }
}

const UUID =
  /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/g;
const PROJECT_ID = /\bp[0-9a-f]{32}\b/g;
const TIMESTAMP_Z = /\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z\b/g;
const TIMESTAMP_LOCAL =
  /\b\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?![\dZ.:+-])/g;
/** A duration derived from a timestamp, only where a field says so: `PAUSED (3s)`, `oldest 2 min`. */
const AGE = /(\bPAUSED \()\d+s\)|(\boldest )\d+( min\b)/g;
/** A pid only where a field says it is one: `pid: 12`, `"pid":12`, `pid 12`. */
const PID = /(\bpid["']?\s*[:=]?\s*)\d+/gi;

/**
 * The normalisation rule. One instance per scenario and client; ids are numbered in order of first appearance, so two
 * runs that name the same ids in the same places read the same, and an id that appears once where the other run repeats
 * one does not.
 */
export class Normaliser {
  readonly #ids: Map<string, number>;

  constructor(
    private readonly root: string,
    private readonly members: Readonly<Record<"pm" | "developer", Member>>,
    ids: Map<string, number> = new Map(),
  ) {
    this.#ids = ids;
  }

  /** A uuid or project id as its ordinal. */
  id(value: string): string {
    let ordinal = this.#ids.get(value);
    if (ordinal === undefined) {
      ordinal = this.#ids.size + 1;
      this.#ids.set(value, ordinal);
    }
    return `<ID#${ordinal}>`;
  }

  text(value: string): string {
    let text = value.replaceAll(this.root, "<ROOT>");
    for (const [name, member] of Object.entries(this.members)) {
      text = text.replaceAll(member.token, `<TOKEN:${name}>`);
      text = text.replaceAll(member.actorId, `<ACTOR:${name}>`);
    }
    return text
      .replace(TIMESTAMP_Z, "<TS:z>")
      .replace(TIMESTAMP_LOCAL, "<TS:local>")
      .replace(UUID, (id) => this.id(id))
      .replace(PROJECT_ID, (id) => this.id(id))
      .replace(PID, "$1<PID>")
      .replace(
        AGE,
        (
          _,
          paused: string | undefined,
          oldest: string | undefined,
          min: string | undefined,
        ) =>
          paused === undefined ? `${oldest}<AGE>${min}` : `${paused}<AGE>s)`,
      );
  }

  observed(run: Observed): Observed {
    return {
      argv: run.argv.map((arg) => this.text(arg)),
      stdout: this.text(run.stdout),
      stderr: this.text(run.stderr),
      exit: run.exit,
    };
  }
}

/** Throws when the Node CLI's run of a step is not what the fixture says it must be. */
export function checkExpectation(
  scenario: string,
  step: Step,
  run: Observed,
): void {
  const expected = step.expect;
  if (expected === undefined) return;
  const where = `${scenario}: cstan ${run.argv.join(" ")}`;
  if (expected.exit !== undefined && run.exit !== expected.exit)
    throw new Error(
      `${where}: exit ${String(run.exit)}, expected ${expected.exit}`,
    );
  const text = `${run.stdout}${run.stderr}`;
  for (const part of expected.contains ?? [])
    if (!text.includes(part))
      throw new Error(
        `${where}: output lacks ${JSON.stringify(part)}: ${text}`,
      );
}

export interface ScenarioResult {
  readonly steps: readonly Observed[];
  readonly ledger: unknown;
}

/** Runs one scenario on a fresh project and returns what the client printed and what the ledger holds afterwards. */
export async function runScenario(
  client: Client,
  scenario: Scenario,
  frontEnd: string,
): Promise<ScenarioResult> {
  const project = await ParityProject.create(
    frontEnd,
    scenario.toml,
    scenario.dirs,
  );
  try {
    const normaliser = project.normaliser();
    const steps: Observed[] = [];
    for (const step of scenario.steps) {
      const side = step.setup === true ? "node" : client;
      const resolved = { ...step, argv: project.resolve(step.argv) };
      const pending = project.run(side, resolved);
      let companion: Promise<Observed> | undefined;
      if (step.during !== undefined) {
        const during = step.during;
        companion = project
          .waitBegun("developer")
          .then(() => project.run("node", during));
      }
      const observed = await pending;
      if (companion !== undefined) await companion;
      if (step.setup !== true) {
        if (client === "node") checkExpectation(scenario.name, step, observed);
        steps.push(normaliser.observed(observed));
      }
    }
    return { steps, ledger: project.ledger(normaliser) };
  } finally {
    project.close();
  }
}

/** The scenarios of the suite, from the fixture file. */
export function loadScenarios(): Scenario[] {
  return JSON.parse(
    readFileSync(path.join(FIXTURE_DIRECTORY, "scenarios.json"), "utf8"),
  ) as Scenario[];
}

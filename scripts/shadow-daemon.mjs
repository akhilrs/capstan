#!/usr/bin/env node
// Shadow run: the Node daemon and the Rust daemon answer the same wire requests over copies of one ledger, and the
// responses and the tables must come out the same.
//
//   node scripts/shadow-daemon.mjs <ledger-or-project>       a stopped project (its directory, its .capstan/state or its
//                                                            controller.sqlite beside .capstan/project.json)
//   node scripts/shadow-daemon.mjs --generate <dir> [--agents N] [--messages N] [--plans N]
//                                                            writes a large scratch project (default 50 agents, 5000
//                                                            messages, 20 plans with reports and integrations)
//   node scripts/shadow-daemon.mjs --self-test               a small generated project, then the shadow run
//
// What it does, in order:
//   1. refuses a source whose daemon is running, and never opens the source in place: the ledger is copied twice with
//      the SQLite backup API (scripts/copy-ledger.mjs, a read-only connection) into /tmp/capstan-shadow-*/{node,rust};
//   2. starts the Node daemon on the first copy with CAPSTAN_LAUNCH=off, sends it a battery of wire requests and
//      records each request with its response (transcript.jsonl);
//   3. starts the Rust daemon (CSTAN_DAEMON_BIN, else the release cstan-daemon of ${CARGO_TARGET_DIR:-rust/target}) on the
//      second copy with CAPSTAN_LAUNCH=off and replays the transcript;
//   4. diffs the responses and a dump of every table. Ids and times differ between two runs, so every uuid and ISO time
//      is replaced by a placeholder numbered in order of appearance: the shape and the order of both sides must agree.
// Exit 0 when nothing differs, 1 when something does (the first differences are printed), 2 on a usage error or a refused
// source. CSTAN_SHADOW_PREFIX changes the directory prefix (default /tmp/capstan-shadow-); the directories are removed
// unless --keep is given.
import { spawn } from "node:child_process";
import { randomBytes, randomUUID } from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { copyLedger } from "./copy-ledger.mjs";

const repositoryRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
);
const dist = (file) =>
  pathToFileURL(path.join(repositoryRoot, "dist", file)).href;

class Refusal extends Error {}

// ---------------------------------------------------------------------------------------------------- the source

/** Where the project, its state directory and its ledger are, or a refusal. */
function resolveSource(argument) {
  let target = path.resolve(argument);
  if (!fs.existsSync(target)) throw new Refusal(`${target} does not exist`);
  let ledger;
  let project;
  const stat = fs.statSync(target);
  if (stat.isFile()) {
    if (path.basename(target) !== "controller.sqlite")
      throw new Refusal(`${target} is not a controller.sqlite`);
    ledger = target;
    target = path.dirname(target);
  } else if (fs.existsSync(path.join(target, "controller.sqlite"))) {
    ledger = path.join(target, "controller.sqlite");
  } else if (
    fs.existsSync(path.join(target, ".capstan", "state", "controller.sqlite"))
  ) {
    project = target;
    ledger = path.join(target, ".capstan", "state", "controller.sqlite");
    target = path.dirname(ledger);
  } else {
    throw new Refusal(`no controller.sqlite in ${target}`);
  }
  const stateDirectory = path.dirname(ledger);
  if (project === undefined) {
    const candidate = path.resolve(stateDirectory, "..", "..");
    if (
      path.basename(stateDirectory) === "state" &&
      fs.existsSync(path.join(candidate, ".capstan", "project.json"))
    )
      project = candidate;
  }
  if (project === undefined)
    throw new Refusal(
      "the ledger needs its project beside it (.capstan/project.json and .capstan/operator.key): the daemon cannot open a ledger without the project's credential",
    );
  for (const file of ["project.json", "operator.key"])
    if (!fs.existsSync(path.join(project, ".capstan", file)))
      throw new Refusal(`${path.join(project, ".capstan", file)} is missing`);
  return { project, stateDirectory, ledger };
}

function pidAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

/** A source whose daemon is running is refused: its pid file names a live process, or its socket answers. */
async function refuseRunning(stateDirectory) {
  const pidFile = path.join(stateDirectory, "daemon.pid");
  if (fs.existsSync(pidFile)) {
    const pid = Number.parseInt(fs.readFileSync(pidFile, "utf8"), 10);
    if (Number.isInteger(pid) && pid > 1 && pidAlive(pid))
      throw new Refusal(
        `a daemon (pid ${pid}) is running on ${stateDirectory}; stop it first (cstan stop)`,
      );
  }
  const socketPath = path.join(stateDirectory, "control.sock");
  if (!fs.existsSync(socketPath)) return;
  const answered = await new Promise((resolve) => {
    const socket = net.createConnection(socketPath);
    socket.once("connect", () => {
      socket.destroy();
      resolve(true);
    });
    socket.once("error", () => resolve(false));
  });
  if (answered)
    throw new Refusal(
      `something is listening on ${socketPath}; stop the daemon of that project first`,
    );
}

// ---------------------------------------------------------------------------------------------------- the copies

async function makeCopy(root, label, source) {
  const directory = path.join(root, label);
  const capstan = path.join(directory, ".capstan");
  fs.mkdirSync(capstan, { recursive: true, mode: 0o700 });
  fs.chmodSync(capstan, 0o700);
  const config = JSON.parse(
    fs.readFileSync(
      path.join(source.project, ".capstan", "project.json"),
      "utf8",
    ),
  );
  config.stateDirectory = path.join(capstan, "state");
  fs.writeFileSync(
    path.join(capstan, "project.json"),
    `${JSON.stringify(config, null, 2)}\n`,
    { mode: 0o600 },
  );
  fs.copyFileSync(
    path.join(source.project, ".capstan", "operator.key"),
    path.join(capstan, "operator.key"),
  );
  fs.chmodSync(path.join(capstan, "operator.key"), 0o600);
  for (const entry of ["capstan.toml", "roles"]) {
    const from = path.join(source.project, entry);
    if (fs.existsSync(from))
      fs.cpSync(from, path.join(directory, entry), { recursive: true });
  }
  const copied = await copyLedger(source.ledger, path.join(capstan, "state"));
  return {
    directory,
    ledger: copied ?? path.join(capstan, "state", "controller.sqlite"),
  };
}

// ---------------------------------------------------------------------------------------------------- the daemons

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** A daemon of the given kind running on a copy; `stop()` asks it to shut down and waits for it to exit. */
async function startDaemon(kind, copy, binary) {
  const { callDaemon } = await import(dist("src/client.js"));
  const environment = { ...process.env, CAPSTAN_LAUNCH: "off" };
  for (const name of Object.keys(environment))
    if (name.startsWith("CAPSTAN_") && name !== "CAPSTAN_LAUNCH")
      delete environment[name];
  delete environment.CSTAN_DAEMON;
  const [command, args] =
    kind === "node"
      ? [
          process.execPath,
          [path.join(repositoryRoot, "dist", "src", "cli.js"), "daemon"],
        ]
      : [binary, []];
  const log = fs.openSync(path.join(copy.directory, `${kind}.log`), "a", 0o600);
  const child = spawn(command, args, {
    cwd: copy.directory,
    env: environment,
    stdio: ["ignore", log, log],
  });
  fs.closeSync(log);
  let exited = false;
  const done = new Promise((resolve) =>
    child.once("exit", (code, signal) => {
      exited = true;
      resolve({ code, signal });
    }),
  );
  const socketPath = path.join(
    copy.directory,
    ".capstan",
    "state",
    "control.sock",
  );
  const credential = fs
    .readFileSync(path.join(copy.directory, ".capstan", "operator.key"), "utf8")
    .trim();
  const deadline = Date.now() + 60_000;
  for (;;) {
    if (exited)
      throw new Error(
        `the ${kind} daemon exited during startup: ${fs
          .readFileSync(path.join(copy.directory, `${kind}.log`), "utf8")
          .split("\n")
          .slice(-5)
          .join(" | ")}`,
      );
    try {
      const pong = await callDaemon(socketPath, credential, "ping", [], 2000);
      if (pong.kind === "response" && pong.response.ok === true) break;
    } catch {
      // not listening yet
    }
    if (Date.now() > deadline) {
      child.kill("SIGTERM");
      throw new Error(`the ${kind} daemon did not answer within 60 seconds`);
    }
    await sleep(100);
  }
  return {
    socketPath,
    credential,
    call: async (credentialOverride, command, args) =>
      (
        await callDaemon(
          socketPath,
          credentialOverride ?? credential,
          command,
          args,
          60_000,
        )
      ).response,
    async stop() {
      if (!exited) {
        await callDaemon(socketPath, credential, "shutdown", [], 10_000).catch(
          () => undefined,
        );
        const result = await Promise.race([
          done,
          sleep(20_000).then(() => undefined),
        ]);
        if (result === undefined) {
          child.kill("SIGTERM");
          await Promise.race([done, sleep(10_000)]);
        }
      }
      return await done;
    },
  };
}

// ---------------------------------------------------------------------------------------------------- the battery

/** The wire requests of the shadow run: operator requests always, agent requests when the project has a tokens file. */
function buildBattery(ledgerFacts, tokens) {
  const requests = [];
  const operator = (command, ...args) =>
    requests.push({ as: "operator", command, args });
  const agent = (name, command, ...args) =>
    requests.push({ as: name, command, args });
  operator("ping");
  operator("status");
  operator("plan", "show");
  for (const planId of ledgerFacts.plans.slice(0, 12))
    operator("plan", "show", planId);
  operator("op", "show");
  operator("op", "grants");
  operator("op", "full-auto", "status");
  operator("inbox");
  for (const agentId of ledgerFacts.agents.slice(0, 6)) {
    operator("send", agentId, `shadow message to ${agentId}`);
  }
  operator("send", "nobody-at-all", "no such recipient");
  operator("send", ledgerFacts.agents[0] ?? "pm-1", "");
  const pending = ledgerFacts.messages.slice(0, 4);
  if (pending[0] !== undefined) operator("cancel", pending[0]);
  if (pending[1] !== undefined) operator("resolve", pending[1], "skip");
  if (pending[2] !== undefined) operator("resolve", pending[2], "nonsense");
  operator("cancel", "no-such-message");
  const first = ledgerFacts.agents[0] ?? "pm-1";
  operator("pause", first, "--reason", "shadow pause");
  operator("pause", first, "--reason", "shadow pause again");
  operator("status");
  operator("resume", first, "--reason", "shadow resume");
  operator("pause", "--reason", "shadow run pause");
  operator("status");
  operator("resume", "--reason", "shadow run resume");
  operator("pause", "no-such-agent", "--reason", "x");
  operator("pause", first);
  operator("peek");
  operator("launch");
  operator("status");
  requests.push({ as: "operator", command: "no-such-command", args: [] });
  requests.push({ as: "wrong", command: "status", args: [] });
  requests.push({ as: "wrong", command: "ping", args: [] });
  for (const [name] of Object.entries(tokens).slice(0, 8)) {
    agent(name, "inbox");
    agent(name, "status");
    agent(name, "plan", "show");
  }
  for (const [name] of Object.entries(tokens).slice(0, 3)) agent(name, "peek");
  operator("status");
  return requests;
}

function readFacts(ledger) {
  const sqlite = process.getBuiltinModule("node:sqlite");
  const database = new sqlite.DatabaseSync(ledger, { readOnly: true });
  const column = (sql) => {
    try {
      return database
        .prepare(sql)
        .all()
        .map((row) => Object.values(row)[0]);
    } catch {
      return [];
    }
  };
  try {
    return {
      agents: column("SELECT agent_id FROM agents ORDER BY agent_id LIMIT 40"),
      plans: column("SELECT plan_id FROM plans ORDER BY sequence LIMIT 40"),
      messages: column(
        "SELECT message_id FROM messages WHERE state IN ('queued','notified') ORDER BY sequence LIMIT 8",
      ),
    };
  } finally {
    database.close();
  }
}

// ---------------------------------------------------------------------------------------------------- the diff

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;
const TIME = /\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z/g;

/** Columns that hold a digest over the request id, which is random in every run. */
const MASKED_COLUMNS = new Set(["request_hash"]);

/** The keys whose value is a clock reading or a process fact, never the same in two runs. */
const VOLATILE_KEYS =
  /^(pid|oldestAgeSeconds|ageSeconds|uptimeSeconds|elapsedMs|ms)$/;

/** Replaces every uuid by a placeholder numbered in order of appearance, every ISO time by <time> and the run's own
 * directories by <project>. */
function normalizer(directories, known) {
  const seen = new Map();
  const text = (value) => {
    let out = value;
    for (const directory of directories)
      out = out.split(directory).join("<project>");
    return out
      .replace(UUID, (id) => {
        // An id the source ledger already held is the same in both copies; only ids a daemon made are numbered.
        if (known.has(id)) return id;
        if (!seen.has(id)) seen.set(id, `<id${seen.size + 1}>`);
        return seen.get(id);
      })
      .replace(TIME, "<time>");
  };
  const walk = (value, key) => {
    if (typeof value === "number" && VOLATILE_KEYS.test(key ?? ""))
      return "<volatile>";
    if (typeof value === "string") return text(value);
    if (Array.isArray(value)) return value.map((item) => walk(item, key));
    if (value !== null && typeof value === "object")
      return Object.fromEntries(
        Object.entries(value).map(([name, item]) => [
          text(name),
          walk(item, name),
        ]),
      );
    return value;
  };
  return (value) => walk(value, undefined);
}

/** A row without the ids a daemon made and without times: rows are put in this order before those ids are numbered, so
 * two ledgers that hold the same rows agree whatever order a table without a rowid lists them in. */
function shape(row, known) {
  return JSON.stringify(row)
    .replace(UUID, (id) => (known.has(id) ? id : "<id>"))
    .replace(TIME, "<time>");
}

function canonical(tables, known) {
  return Object.fromEntries(
    Object.entries(tables).map(([name, rows]) => [
      name,
      rows
        .map((row) => ({
          row,
          key: shape(row, known),
          // Rows alike but for their ids come out in the order they were written.
          written: String(row.created_at ?? ""),
        }))
        .sort(
          (a, b) =>
            (a.key < b.key ? -1 : a.key > b.key ? 1 : 0) ||
            (a.written < b.written ? -1 : a.written > b.written ? 1 : 0),
        )
        .map(({ row }) => row),
    ]),
  );
}

function dumpTables(ledger) {
  const sqlite = process.getBuiltinModule("node:sqlite");
  const database = new sqlite.DatabaseSync(ledger, { readOnly: true });
  try {
    const names = database
      .prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
      )
      .all()
      .map((row) => row.name);
    const tables = {};
    for (const name of names) {
      let rows;
      try {
        rows = database.prepare(`SELECT * FROM "${name}" ORDER BY rowid`).all();
      } catch {
        rows = database.prepare(`SELECT * FROM "${name}"`).all();
      }
      if (rows.length === 0) continue;
      tables[name] = rows.map((row) =>
        Object.fromEntries(
          Object.entries(row).map(([key, value]) => [
            key,
            MASKED_COLUMNS.has(key)
              ? "<digest>"
              : value instanceof Uint8Array
                ? `blob:${Buffer.from(value).toString("hex")}`
                : typeof value === "bigint"
                  ? Number(value)
                  : value,
          ]),
        ),
      );
    }
    return tables;
  } finally {
    database.close();
  }
}

/** The first differences between two normalized values, as "path: left != right" lines. */
function differences(left, right, where = "", out = [], limit = 30) {
  if (out.length >= limit) return out;
  if (JSON.stringify(left) === JSON.stringify(right)) return out;
  if (
    left !== null &&
    right !== null &&
    typeof left === "object" &&
    typeof right === "object" &&
    Array.isArray(left) === Array.isArray(right)
  ) {
    const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
    for (const key of keys)
      differences(left[key], right[key], `${where}/${key}`, out, limit);
    return out;
  }
  out.push(
    `${where}: ${JSON.stringify(left)?.slice(0, 200)} != ${JSON.stringify(right)?.slice(0, 200)}`,
  );
  return out;
}

// ---------------------------------------------------------------------------------------------------- the run

async function shadow(sourceArgument, options) {
  const source = resolveSource(sourceArgument);
  await refuseRunning(source.stateDirectory);
  const binary =
    process.env.CSTAN_DAEMON_BIN ??
    path.join(
      path.resolve(
        repositoryRoot,
        "rust",
        process.env.CARGO_TARGET_DIR ?? "target",
      ),
      "release",
      "cstan-daemon",
    );
  if (!path.isAbsolute(binary) || !fs.existsSync(binary))
    throw new Refusal(
      `${binary} does not exist; build it (npm run check:dash) or set CSTAN_DAEMON_BIN`,
    );
  if (!fs.existsSync(path.join(repositoryRoot, "dist", "src", "cli.js")))
    throw new Refusal("dist/ is missing; run npm run build");
  const prefix = process.env.CSTAN_SHADOW_PREFIX ?? "/tmp/capstan-shadow-";
  const root = fs.mkdtempSync(prefix);
  fs.chmodSync(root, 0o700);
  try {
    const nodeCopy = await makeCopy(root, "node", source);
    const rustCopy = await makeCopy(root, "rust", source);
    // Everything the run learns of the ledger it learns from a copy: the source is only ever read by the backup.
    const facts = readFacts(rustCopy.ledger);
    const tokensFile = path.join(source.project, "shadow-tokens.json");
    const tokens = fs.existsSync(tokensFile)
      ? JSON.parse(fs.readFileSync(tokensFile, "utf8"))
      : {};
    const known = new Set(
      JSON.stringify(dumpTables(rustCopy.ledger)).match(UUID) ?? [],
    );
    const battery = buildBattery(facts, tokens);
    const credentialFor = (daemon, who) =>
      who === "operator"
        ? daemon.credential
        : who === "wrong"
          ? "wrong-credential-0123456789-0123456789"
          : tokens[who];

    const node = await startDaemon("node", nodeCopy, binary);
    const recorded = [];
    for (const request of battery) {
      const response = await node.call(
        credentialFor(node, request.as),
        request.command,
        request.args,
      );
      recorded.push({ ...request, response });
    }
    const nodeExit = await node.stop();
    fs.writeFileSync(
      path.join(root, "transcript.jsonl"),
      recorded.map((entry) => JSON.stringify(entry)).join("\n") + "\n",
    );

    const rust = await startDaemon("rust", rustCopy, binary);
    const replayed = [];
    for (const request of recorded) {
      const response = await rust.call(
        credentialFor(rust, request.as),
        request.command,
        request.args,
      );
      replayed.push({ ...request, response });
    }
    const rustExit = await rust.stop();

    const left = normalizer([nodeCopy.directory, rustCopy.directory], known);
    const right = normalizer([nodeCopy.directory, rustCopy.directory], known);
    const responsesNode = recorded.map((entry) => left(entry.response));
    const responsesRust = replayed.map((entry) => right(entry.response));
    const tablesNode = left(canonical(dumpTables(nodeCopy.ledger), known));
    const tablesRust = right(canonical(dumpTables(rustCopy.ledger), known));

    const found = [];
    for (let index = 0; index < recorded.length; index += 1) {
      for (const line of differences(
        responsesNode[index],
        responsesRust[index],
      ))
        found.push(
          `response ${index} (${recorded[index].as}: ${recorded[index].command} ${recorded[index].args.join(" ")}) ${line}`,
        );
    }
    for (const line of differences(tablesNode, tablesRust, "", [], 60))
      found.push(`table ${line}`);
    for (const [kind, exit] of [
      ["node", nodeExit],
      ["rust", rustExit],
    ])
      if (exit.code !== 0)
        found.push(
          `the ${kind} daemon exited with ${exit.code ?? exit.signal}`,
        );

    const rows = Object.values(tablesNode).reduce(
      (sum, table) => sum + table.length,
      0,
    );
    const summary = `shadow: ${recorded.length} requests, ${Object.keys(tablesNode).length} tables, ${rows} rows; source ${source.ledger}`;
    if (found.length === 0) {
      process.stdout.write(
        `${summary}\nshadow: no response and no table differs\n`,
      );
      return 0;
    }
    process.stdout.write(
      `${summary}\nshadow: ${found.length}${found.length >= 30 ? "+" : ""} difference(s):\n`,
    );
    for (const line of found.slice(0, 60)) process.stdout.write(`  ${line}\n`);
    options.keep = true;
    return 1;
  } finally {
    if (options.keep) process.stdout.write(`shadow: kept ${root}\n`);
    else fs.rmSync(root, { recursive: true, force: true });
  }
}

// ---------------------------------------------------------------------------------------------------- the generator

/** Writes a scratch project with a large ledger: agents, messages, and plans with reports and integrations. */
async function generate(directory, { agents, messages, plans }) {
  const { ControllerCore } = await import(dist("src/controller/core.js"));
  const { PLACEHOLDER_INPUTS } = await import(dist("src/daemon.js"));
  const project = path.resolve(directory);
  if (!path.resolve(project).startsWith("/tmp/"))
    throw new Refusal("the generated project must be under /tmp");
  const capstan = path.join(project, ".capstan");
  fs.mkdirSync(capstan, { recursive: true, mode: 0o700 });
  fs.chmodSync(capstan, 0o700);
  const stateDirectory = path.join(capstan, "state");
  const projectId = `p${randomUUID().replaceAll("-", "")}`;
  const owner = randomBytes(24).toString("hex");
  fs.writeFileSync(
    path.join(capstan, "project.json"),
    `${JSON.stringify(
      {
        schemaVersion: 1,
        projectId,
        name: "Shadow project",
        stateDirectory,
        maxSlices: 4,
        maxRunMs: 3600000,
        maxDispatches: 16,
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  fs.writeFileSync(path.join(capstan, "operator.key"), `${owner}\n`, {
    mode: 0o600,
  });
  const core = await ControllerCore.open({
    stateDirectory,
    project: {
      projectId,
      name: "Shadow project",
      ownerCredential: owner,
      initialInputs: PLACEHOLDER_INPUTS,
    },
  });
  const tokens = {};
  const ctx = (credential) => {
    const n = randomUUID();
    return {
      credential,
      requestId: `req-${n}`,
      idempotencyKey: `idem-${n}`,
      expectedVersion: core.stateVersion,
      inputRevision: core.inputRevision,
    };
  };
  const hash = (letter) => letter.repeat(64);
  const branchOf = (agentId) => `feat/${agentId}`;
  const baseSha = "0123456789abcdef0123456789abcdef01234567";
  const sha = (n) => n.toString(16).padStart(40, "0");
  try {
    core.syncRoleDefinitions(ctx(owner), [
      { name: "pm", kind: "PM", host: "claude", configHash: hash("b") },
      {
        name: "developer",
        kind: "Developer",
        host: "claude",
        configHash: hash("a"),
      },
      {
        name: "reviewer",
        kind: "Verifier",
        host: "claude",
        configHash: hash("c"),
      },
    ]);
    const member = (agentId, roleName, kind) => {
      const seatId = `seat-${agentId}`;
      core.createSeat(ctx(owner), { seatId, name: seatId, role: kind });
      const actor = core.createActor(ctx(owner), {
        displayName: agentId,
        role: kind,
        seatId,
      });
      core.registerAgent(ctx(owner), {
        agentId,
        roleName,
        seatId,
        actorId: actor.actorId,
      });
      tokens[agentId] = actor.credential;
      if (kind === "Developer")
        core.recordAgentPane(ctx(owner), {
          agentId,
          workspaceId: null,
          paneId: null,
          worktreePath: null,
          branch: branchOf(agentId),
          baseSha,
        });
      return actor.credential;
    };
    const pm = member("pm-1", "pm", "PM");
    const reviewer = member("rev-1", "reviewer", "Verifier");
    const architect = member("arch-1", "developer", "Developer");
    const developers = [];
    for (let index = 1; index <= Math.max(agents - 3, plans * 2); index += 1) {
      const agentId = `dev-${index}`;
      member(agentId, "developer", "Developer");
      developers.push(agentId);
    }
    const body = JSON.stringify({
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
          id: "ui",
          title: "UI",
          owns: ["src/ui"],
          interfaces: ["ui api"],
          dependsOn: ["core"],
          estimateHours: 2,
          acceptance: ["ui works"],
          risks: ["none"],
          type: "feat",
          scope: "ui",
        },
      ],
      integrationOrder: ["core", "ui"],
    });
    const evidence = (agentId, commit) => ({
      baseSha,
      branch: branchOf(agentId),
      branchTip: commit,
      checkedAt: "2026-10-01T00:00:00.000Z",
      commitExists: true,
      generation: 1,
      isAncestorOfBase: false,
      isAncestorOfTip: true,
    });
    let counter = 0;
    for (let n = 1; n <= plans; n += 1) {
      const plan = core.openPlan(ctx(pm), {
        tier: "normal",
        title: `Plan ${n}`,
      });
      const planId = plan.planId;
      core.submitPlan(ctx(architect), {
        planId,
        bodyJson: body,
        baseSha,
        review: true,
      });
      core.beginReview(ctx(pm), {
        subjectId: planId,
        reviewerRole: "reviewer",
        reviewerAgentId: "rev-1",
      });
      core.completeReview(ctx(reviewer), {
        verdict: "pass",
        text: "The plan holds.",
      });
      const [first, second] = [
        developers[2 * (n - 1)],
        developers[2 * (n - 1) + 1],
      ];
      core.assignPackage(ctx(pm), {
        planId,
        packageId: "core",
        agentId: first,
      });
      core.assignPackage(ctx(pm), {
        planId,
        packageId: "ui",
        agentId: second,
        early: "the interface is stated in the plan",
      });
      const reportIds = [];
      for (const agentId of [first, second]) {
        counter += 1;
        const commit = sha(counter);
        const report = core.recordAgentReport(ctx(tokens[agentId]), {
          commitSha: commit,
          summary: `Work of ${agentId}`,
          evidence: evidence(agentId, commit),
        });
        reportIds.push(report.record.reportId);
        core.beginReview(ctx(architect), {
          subjectId: report.record.reportId,
          reviewerRole: "reviewer",
          reviewerAgentId: "rev-1",
        });
        core.completeReview(ctx(reviewer), {
          verdict: "pass",
          text: `${agentId} is fine.`,
        });
      }
      const integrationId = `int-${n}`;
      core.beginIntegration(ctx(owner), {
        integrationId,
        reportIds,
        baseSha,
        branch: `capstan/integration/${integrationId}`,
        requestedBy: "pm",
      });
      core.finishIntegration(ctx(owner), {
        integrationId,
        outcome: { kind: "merged", headSha: sha(10_000 + n) },
      });
      core.beginReview(ctx(architect), {
        subjectId: integrationId,
        reviewerRole: "reviewer",
        reviewerAgentId: "rev-1",
      });
      core.completeReview(ctx(reviewer), {
        verdict: "pass",
        text: "The integration is fine.",
      });
      core.recordSignoff(ctx(architect), {
        planId,
        integrationId,
        summary: "Everything is in.",
      });
      core.settleIntegration(ctx(owner), {
        integrationId,
        outcome: "confirmed",
      });
    }
    const recipients = ["pm-1", "rev-1", "arch-1", ...developers];
    for (let index = 0; index < messages; index += 1)
      core.enqueueMessage(ctx(owner), {
        recipientAgentId: recipients[index % recipients.length],
        body: `shadow message ${index + 1}`,
      });
  } finally {
    core.close();
  }
  fs.writeFileSync(
    path.join(project, "capstan.toml"),
    `schema_version = 1

[nexora]
track = "never"

[architect]
enabled = true
role = "developer"

[hosts.claude]
kind = "claude"

[roles.pm]
kind = "PM"
host = "claude"

[roles.developer]
kind = "Developer"
host = "claude"

[roles.reviewer]
kind = "Verifier"
host = "claude"
`,
    { mode: 0o600 },
  );
  fs.writeFileSync(
    path.join(project, "shadow-tokens.json"),
    JSON.stringify(tokens),
    { mode: 0o600 },
  );
  return project;
}

// ---------------------------------------------------------------------------------------------------- main

function usage(message) {
  process.stderr.write(
    `${message === undefined ? "" : `shadow-daemon: ${message}\n`}usage: node scripts/shadow-daemon.mjs <ledger-or-project> [--keep]\n       node scripts/shadow-daemon.mjs --generate <dir> [--agents N] [--messages N] [--plans N]\n       node scripts/shadow-daemon.mjs --self-test [--keep]\n`,
  );
  return 2;
}

async function main(argv) {
  const options = { keep: false };
  const positional = [];
  const numbers = { agents: 50, messages: 5000, plans: 20 };
  let mode = "shadow";
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--keep") options.keep = true;
    else if (argument === "--generate") mode = "generate";
    else if (argument === "--self-test") mode = "self-test";
    else if (["--agents", "--messages", "--plans"].includes(argument)) {
      const value = Number.parseInt(argv[(index += 1)] ?? "", 10);
      if (!Number.isInteger(value) || value < 1)
        return usage(`${argument} needs a positive integer`);
      numbers[argument.slice(2)] = value;
    } else if (argument.startsWith("--"))
      return usage(`unknown option ${argument}`);
    else positional.push(argument);
  }
  try {
    if (mode === "generate") {
      if (positional.length !== 1)
        return usage("--generate takes one directory");
      const project = await generate(positional[0], numbers);
      process.stdout.write(`${project}\n`);
      return 0;
    }
    if (mode === "self-test") {
      if (positional.length !== 0) return usage("--self-test takes no source");
      const scratch = fs.mkdtempSync(
        `${process.env.CSTAN_SHADOW_PREFIX ?? "/tmp/capstan-shadow-"}generated-`,
      );
      try {
        const project = await generate(path.join(scratch, "project"), {
          agents: 8,
          messages: 120,
          plans: 3,
        });
        return await shadow(project, options);
      } finally {
        fs.rmSync(scratch, { recursive: true, force: true });
      }
    }
    if (positional.length !== 1) return usage();
    return await shadow(positional[0], options);
  } catch (error) {
    if (error instanceof Refusal) {
      process.stderr.write(`shadow-daemon: ${error.message}\n`);
      return 2;
    }
    throw error;
  }
}

process.exitCode = await main(process.argv.slice(2));

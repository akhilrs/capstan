import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  chmodSync,
  closeSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import Database from "better-sqlite3";
import {
  callDaemon,
  openDaemonLog,
  pingDaemon,
  scrubEnvironment,
} from "../src/client.js";
import {
  MAX_FRAME_BYTES,
  ROUTES,
  defaultVerificationHooks,
  removeStaleSocket,
  startDaemonServer,
  type CommandResponse,
  type DaemonServer,
  type LogEntry,
} from "../src/daemon.js";
import { ControllerCore } from "../src/controller/core.js";
import type {
  InitialProject,
  MutationContext,
} from "../src/controller/types.js";

const inputKinds = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
] as const;

function projectInfo(): InitialProject {
  const identity = crypto.randomUUID().replaceAll("-", "");
  return {
    projectId: `p${identity}`,
    name: "Daemon test project",
    ownerCredential: `owner-${identity}`,
    initialInputs: inputKinds.map((kind) => ({
      kind,
      content:
        kind === "acceptance_criteria" ? ["criterion"] : { kind, revision: 1 },
    })),
  };
}

function ctx(core: ControllerCore, credential: string): MutationContext {
  const id = crypto.randomUUID();
  return {
    credential,
    requestId: `req-${id}`,
    idempotencyKey: `idem-${id}`,
    expectedVersion: core.stateVersion,
    inputRevision: core.inputRevision,
  };
}

interface Member {
  readonly agentId: string;
  readonly credential: string;
  readonly actorId: string;
}

interface Harness {
  readonly core: ControllerCore;
  readonly stateDirectory: string;
  readonly socketPath: string;
  readonly info: InitialProject;
  readonly owner: string;
  readonly pm: Member;
  readonly developer: Member;
  readonly seatOnly: string;
  readonly log: LogEntry[];
  readonly shutdowns: number[];
  readonly server: DaemonServer;
}

async function harness(): Promise<Harness> {
  const stateDirectory = mkdtempSync(path.join(tmpdir(), "capstan-daemon-"));
  const info = projectInfo();
  const core = await ControllerCore.open({ stateDirectory, project: info });
  const owner = info.ownerCredential;
  core.syncRoleDefinitions(ctx(core, owner), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
    {
      name: "developer",
      kind: "Developer",
      host: "claude",
      configHash: "b".repeat(64),
    },
  ]);
  const member = (name: string, kind: "PM" | "Developer"): Member => {
    const seatId = `${name}-seat`;
    core.createSeat(ctx(core, owner), { seatId, name, role: kind });
    const actor = core.createActor(ctx(core, owner), {
      displayName: name,
      role: kind,
      seatId,
    });
    const agentId = `${name}-agent`;
    core.registerAgent(ctx(core, owner), {
      agentId,
      roleName: name,
      seatId,
      actorId: actor.actorId,
    });
    return { agentId, credential: actor.credential, actorId: actor.actorId };
  };
  const pm = member("pm", "PM");
  const developer = member("developer", "Developer");
  core.createSeat(ctx(core, owner), {
    seatId: "loose-seat",
    name: "loose",
    role: "Verifier",
  });
  const loose = core.createActor(ctx(core, owner), {
    displayName: "loose",
    role: "Verifier",
    seatId: "loose-seat",
  });
  const socketPath = path.join(stateDirectory, "control.sock");
  const log: LogEntry[] = [];
  const shutdowns: number[] = [];
  const server = await startDaemonServer({
    socketPath,
    core,
    log: (entry) => log.push(entry),
    onShutdown: () => shutdowns.push(Date.now()),
  });
  return {
    core,
    stateDirectory,
    socketPath,
    info,
    owner,
    pm,
    developer,
    seatOnly: loose.credential,
    log,
    shutdowns,
    server,
  };
}

async function close(h: Harness): Promise<void> {
  await h.server.close();
  h.core.close();
  rmSync(h.stateDirectory, { recursive: true, force: true });
}

async function call(
  h: Harness,
  credential: string,
  command: string,
  args: string[] = [],
): Promise<CommandResponse> {
  const result = await callDaemon(h.socketPath, credential, command, args);
  assert.equal(result.kind, "response");
  return (result as { response: CommandResponse }).response;
}

function rawExchange(
  socketPath: string,
  payload: Buffer,
  timeoutMs = 3000,
): Promise<string> {
  return new Promise((resolve) => {
    const socket = net.createConnection(socketPath);
    let data = "";
    socket.on("connect", () => socket.write(payload));
    socket.on("data", (chunk) => (data += chunk.toString("utf8")));
    socket.on("close", () => resolve(data));
    socket.on("error", () => resolve(data));
    socket.setTimeout(timeoutMs, () => {
      socket.destroy();
      resolve(data);
    });
  });
}

const READ = Object.keys(ROUTES).filter(
  (name) => ROUTES[name]!.access === "read",
);
const AGENT = Object.keys(ROUTES).filter(
  (name) => ROUTES[name]!.access === "agent",
);
const OPERATOR = Object.keys(ROUTES).filter(
  (name) => ROUTES[name]!.access === "operator" && name !== "shutdown",
);

function code(response: CommandResponse): string {
  return response.ok ? "ok" : response.code;
}

test("the operator credential is accepted for operator and read commands and refused for agent commands", async () => {
  const h = await harness();
  try {
    for (const name of READ)
      assert.equal(code(await call(h, h.owner, name)), "ok", name);
    for (const name of OPERATOR)
      assert.equal(code(await call(h, h.owner, name)), "not_implemented", name);
    for (const name of AGENT)
      assert.equal(code(await call(h, h.owner, name)), "forbidden", name);
  } finally {
    await close(h);
  }
});

test("an agent token is accepted for agent and read commands and refused for operator commands", async () => {
  const h = await harness();
  try {
    for (const member of [h.pm, h.developer]) {
      for (const name of READ)
        assert.equal(code(await call(h, member.credential, name)), "ok", name);
      for (const name of AGENT)
        assert.equal(
          code(await call(h, member.credential, name)),
          "not_implemented",
          name,
        );
      for (const name of [...OPERATOR, "shutdown"])
        assert.equal(
          code(await call(h, member.credential, name)),
          "forbidden",
          name,
        );
    }
    assert.deepEqual(h.shutdowns, []);
  } finally {
    await close(h);
  }
});

test("a seat actor that is not an agent and the internal controller are refused everywhere", async () => {
  const h = await harness();
  try {
    for (const name of Object.keys(ROUTES))
      assert.equal(code(await call(h, h.seatOnly, name)), "forbidden", name);
    assert.equal(
      code(await call(h, h.seatOnly, "no-such-command")),
      "forbidden",
    );
  } finally {
    await close(h);
  }
});

test("a missing, malformed or wrong credential is unauthorized for every command", async () => {
  const h = await harness();
  try {
    const names = [...Object.keys(ROUTES), "no-such-command"];
    for (const credential of [
      "",
      "short",
      "x".repeat(64),
      "y".repeat(300),
      "a\ud800b".repeat(20),
    ])
      for (const name of names) {
        const response = await call(h, credential, name);
        assert.equal(
          code(response),
          "unauthorized",
          `${name} with ${credential.length}`,
        );
        assert.ok(
          !JSON.stringify(response).includes(credential) || credential === "",
        );
      }
    const missing = await rawExchange(
      h.socketPath,
      Buffer.from('{"v":1,"command":"ping"}\n'),
    );
    assert.equal(
      (JSON.parse(missing) as { code: string }).code,
      "unauthorized",
    );
    const numeric = await rawExchange(
      h.socketPath,
      Buffer.from('{"v":1,"credential":42,"command":"ping"}\n'),
    );
    assert.equal(
      (JSON.parse(numeric) as { code: string }).code,
      "unauthorized",
    );
    assert.equal(
      code(await call(h, h.owner, "no-such-command")),
      "unknown_command",
    );
    assert.equal(
      code(await call(h, h.pm.credential, "no-such-command")),
      "unknown_command",
    );
  } finally {
    await close(h);
  }
});

test("shutdown needs the operator credential and reaches the shutdown hook once", async () => {
  const h = await harness();
  try {
    assert.equal(
      code(await call(h, h.developer.credential, "shutdown")),
      "forbidden",
    );
    assert.equal(h.shutdowns.length, 0);
    const response = await call(h, h.owner, "shutdown");
    assert.deepEqual(response, { ok: true, result: { stopping: true } });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(h.shutdowns.length, 1);
  } finally {
    await close(h);
  }
});

test("tokens are per agent and per generation and never appear in status, the log or the ledger", async () => {
  const h = await harness();
  try {
    const { core, pm, developer, owner } = h;
    assert.notEqual(pm.credential, developer.credential);
    const identities = [pm, developer].map((member) =>
      core.identify(member.credential),
    );
    assert.deepEqual(
      identities.map((identity) => identity.agent?.agentId),
      [pm.agentId, developer.agentId],
    );
    const replaced = core.replaceAgentGeneration(
      ctx(core, owner),
      developer.agentId,
    );
    assert.notEqual(replaced.credential, developer.credential);
    assert.equal(
      code(await call(h, developer.credential, "status")),
      "unauthorized",
    );
    const fresh = await call(h, replaced.credential, "status");
    assert.equal(code(fresh), "ok");
    const status = (
      fresh as {
        result: { agents: Array<{ agentId: string; generation: number }> };
      }
    ).result;
    assert.equal(
      status.agents.find((agent) => agent.agentId === developer.agentId)
        ?.generation,
      2,
    );

    const secrets = [
      owner,
      pm.credential,
      developer.credential,
      replaced.credential,
      h.seatOnly,
    ];
    const outputs = [
      JSON.stringify((await call(h, owner, "status")) as object),
      JSON.stringify((await call(h, pm.credential, "status")) as object),
      JSON.stringify(h.log),
    ];
    const db = new Database(path.join(h.stateDirectory, "controller.sqlite"), {
      readonly: true,
    });
    try {
      const tables = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .all() as Array<{ name: string }>;
      for (const { name } of tables)
        outputs.push(JSON.stringify(db.prepare(`SELECT * FROM ${name}`).all()));
    } finally {
      db.close();
    }
    for (const secret of secrets)
      for (const text of outputs)
        assert.ok(
          !text.includes(secret),
          "a credential appeared in plain text",
        );
  } finally {
    await close(h);
  }
});

test("the log records command, actor and result but never arguments or credentials", async () => {
  const h = await harness();
  try {
    await call(h, h.owner, "send", ["dev", "SECRET-MESSAGE-TEXT"]);
    await call(h, "z".repeat(40), "status");
    const sendEntry = h.log.find((entry) => entry.command === "send")!;
    assert.equal(sendEntry.role, "operator");
    assert.equal(sendEntry.code, "not_implemented");
    assert.equal(sendEntry.argCount, 2);
    assert.equal(
      sendEntry.argBytes,
      "dev".length + "SECRET-MESSAGE-TEXT".length,
    );
    const denied = h.log.find((entry) => entry.code === "unauthorized")!;
    assert.equal(denied.actorId, null);
    const text = JSON.stringify(h.log);
    assert.ok(!text.includes("SECRET-MESSAGE-TEXT"));
    assert.ok(!text.includes("z".repeat(40)));
  } finally {
    await close(h);
  }
});

test("the socket is a socket with mode 0600 in a private state directory", async () => {
  const h = await harness();
  try {
    const stat = statSync(h.socketPath);
    assert.ok(stat.isSocket());
    assert.equal(stat.mode & 0o777, 0o600);
    assert.equal(statSync(h.stateDirectory).mode & 0o077, 0);
  } finally {
    await close(h);
  }
});

function deadSocket(socketPath: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const child = spawn(
      process.execPath,
      [
        "-e",
        `const net=require("node:net");net.createServer().listen(process.argv[1],()=>console.log("up"));setInterval(()=>{},1000)`,
        socketPath,
      ],
      { stdio: ["ignore", "pipe", "ignore"] },
    );
    child.stdout.once("data", () => {
      child.kill("SIGKILL");
      child.once("exit", () => resolve());
    });
    child.once("error", reject);
  });
}

test("a stale socket from a dead process is replaced and the new server answers", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-daemon-stale-"),
  );
  const info = projectInfo();
  const socketPath = path.join(stateDirectory, "control.sock");
  const core = await ControllerCore.open({ stateDirectory, project: info });
  let server: DaemonServer | undefined;
  try {
    await deadSocket(socketPath);
    assert.ok(statSync(socketPath).isSocket());
    server = await startDaemonServer({
      socketPath,
      core,
      log: () => {},
      onShutdown: () => {},
    });
    assert.deepEqual(await pingDaemon(socketPath, info.ownerCredential), {
      outcome: "running",
      pid: process.pid,
    });
    assert.equal(statSync(socketPath).mode & 0o777, 0o600);
  } finally {
    await server?.close();
    core.close();
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});

test("a non-socket or a symlink at the socket path is refused and left alone", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-daemon-file-"),
  );
  const socketPath = path.join(stateDirectory, "control.sock");
  try {
    writeFileSync(socketPath, "not a socket");
    assert.throws(() => removeStaleSocket(socketPath), /is not a socket/);
    assert.equal(readFileSync(socketPath, "utf8"), "not a socket");
    rmSync(socketPath);
    writeFileSync(path.join(stateDirectory, "target"), "x");
    symlinkSync("target", socketPath);
    assert.throws(() => removeStaleSocket(socketPath), /is not a socket/);
    assert.throws(() => removeStaleSocket("relative.sock"), TypeError);
    chmodSync(stateDirectory, 0o755);
    rmSync(socketPath);
    assert.throws(() => removeStaleSocket(socketPath), /must be private/);
  } finally {
    chmodSync(stateDirectory, 0o700);
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});

test("frames: exactly the limit is accepted, one byte more is dropped, bad input is rejected", async () => {
  const h = await harness();
  try {
    const withPadding = (size: number): Buffer => {
      const base = JSON.stringify({
        v: 1,
        credential: h.owner,
        command: "ping",
        args: [],
        pad: "",
      });
      const padding = "p".repeat(size - Buffer.byteLength(base));
      return Buffer.from(base.replace('"pad":""', `"pad":"${padding}"`));
    };
    const exact = withPadding(MAX_FRAME_BYTES);
    assert.equal(exact.length, MAX_FRAME_BYTES);
    const ok = await rawExchange(
      h.socketPath,
      Buffer.concat([exact, Buffer.from("\n")]),
    );
    assert.equal((JSON.parse(ok) as { ok: boolean }).ok, true);
    const over = withPadding(MAX_FRAME_BYTES + 1);
    assert.equal(
      await rawExchange(h.socketPath, Buffer.concat([over, Buffer.from("\n")])),
      "",
    );
    assert.equal(await rawExchange(h.socketPath, over), "");
    for (const bad of [
      Buffer.from("not json\n"),
      Buffer.from([0xff, 0xfe, 0x0a]),
      Buffer.from("[]\n"),
      Buffer.from('{"v":2,"credential":"x","command":"ping"}\n'),
      Buffer.from('{"v":1,"credential":"x","command":""}\n'),
      Buffer.from(`{"v":1,"credential":"x","command":"${"c".repeat(65)}"}\n`),
    ])
      assert.equal(
        (JSON.parse(await rawExchange(h.socketPath, bad)) as { code: string })
          .code,
        "invalid_request",
      );
    const tooMany = JSON.stringify({
      v: 1,
      credential: h.owner,
      command: "ping",
      args: Array(17).fill("a"),
    });
    assert.equal(
      (
        JSON.parse(
          await rawExchange(h.socketPath, Buffer.from(`${tooMany}\n`)),
        ) as { code: string }
      ).code,
      "invalid_request",
    );
    const nonString = JSON.stringify({
      v: 1,
      credential: h.owner,
      command: "ping",
      args: [1],
    });
    assert.equal(
      (
        JSON.parse(
          await rawExchange(h.socketPath, Buffer.from(`${nonString}\n`)),
        ) as { code: string }
      ).code,
      "invalid_request",
    );
    assert.equal(code(await call(h, h.owner, "ping")), "ok");
  } finally {
    await close(h);
  }
});

test("a client cannot send a request larger than the frame limit", async () => {
  const h = await harness();
  try {
    await assert.rejects(
      callDaemon(h.socketPath, h.owner, "ping", ["x".repeat(MAX_FRAME_BYTES)]),
      TypeError,
    );
  } finally {
    await close(h);
  }
});

test("the legacy status and inspect requests work for the operator token and nothing else", async () => {
  const h = await harness();
  try {
    const legacy = (body: object) =>
      rawExchange(h.socketPath, Buffer.from(`${JSON.stringify(body)}\n`)).then(
        (text) => JSON.parse(text) as Record<string, unknown>,
      );
    const status = await legacy({ token: h.owner, action: "status" });
    assert.ok("result" in status && "requestId" in status);
    assert.equal(
      (status.result as { projectId: string }).projectId,
      h.info.projectId,
    );
    assert.deepEqual(
      await legacy({ token: h.pm.credential, action: "status" }),
      { error: "unauthorized" },
    );
    assert.deepEqual(await legacy({ token: "nope", action: "status" }), {
      error: "unauthorized",
    });
    assert.deepEqual(await legacy({ action: "status" }), {
      error: "unauthorized",
    });
    for (const action of ["pause", "resume", "cancel"])
      assert.deepEqual(await legacy({ token: h.owner, action }), {
        error:
          "pause, resume and cancel require the foreground cstan run controller",
      });
    assert.deepEqual(await legacy({ token: h.owner, action: "bogus" }), {
      error: "invalid control request",
    });
    const missing = await legacy({
      token: h.owner,
      action: "inspect",
      id: "no-such-id",
    });
    assert.ok(typeof missing.error === "string");
  } finally {
    await close(h);
  }
});

test("pingDaemon tells a running daemon, a legacy controller, a refusal, no daemon and a silent one apart", async () => {
  const h = await harness();
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-ping-"));
  const servers: net.Server[] = [];
  try {
    assert.equal((await pingDaemon(h.socketPath, h.owner)).outcome, "running");
    assert.deepEqual(await pingDaemon(h.socketPath, "w".repeat(40)), {
      outcome: "refused",
      code: "unauthorized",
    });
    assert.deepEqual(
      await pingDaemon(path.join(directory, "none.sock"), h.owner),
      { outcome: "down" },
    );

    const serve = async (
      name: string,
      handler: (socket: net.Socket) => void,
    ): Promise<string> => {
      const socketPath = path.join(directory, name);
      const server = net.createServer(handler);
      servers.push(server);
      await new Promise<void>((resolve) => server.listen(socketPath, resolve));
      return socketPath;
    };
    const legacy = await serve("legacy.sock", (socket) => {
      socket.once("data", () => socket.end('{"error":"unauthorized"}\n'));
    });
    assert.deepEqual(await pingDaemon(legacy, h.owner), { outcome: "legacy" });
    const silent = await serve("silent.sock", () => {});
    assert.equal(
      (await pingDaemon(silent, h.owner, 300)).outcome,
      "unreachable",
    );
    const reset = await serve("reset.sock", (socket) => socket.destroy());
    assert.equal((await pingDaemon(reset, h.owner)).outcome, "unreachable");
    const garbage = await serve("garbage.sock", (socket) => {
      socket.once("data", () => socket.end("nonsense\n"));
    });
    assert.equal((await pingDaemon(garbage, h.owner)).outcome, "unreachable");
  } finally {
    for (const server of servers) server.close();
    rmSync(directory, { recursive: true, force: true });
    await close(h);
  }
});

test("the default verification hooks answer the generation check and leave the rest unsupported", async () => {
  const h = await harness();
  try {
    const hooks = defaultVerificationHooks(h.core);
    assert.equal(hooks.generationIsCurrent(h.pm.agentId, 1), true);
    assert.equal(hooks.generationIsCurrent(h.pm.agentId, 2), false);
    assert.equal(hooks.generationIsCurrent("nobody", 1), false);
    h.core.replaceAgentGeneration(ctx(h.core, h.owner), h.pm.agentId);
    assert.equal(hooks.generationIsCurrent(h.pm.agentId, 1), false);
    assert.equal(hooks.generationIsCurrent(h.pm.agentId, 2), true);
    assert.equal(hooks.commitOnAgentBranch(h.pm.agentId, "abc"), undefined);
    assert.equal(hooks.transitionLegal("message", "queued", "sent"), undefined);
  } finally {
    await close(h);
  }
});

test("the daemon log file must be a private regular file and the spawn environment loses the agent variables", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-log-"));
  try {
    const logPath = path.join(directory, "daemon.log");
    closeSync(openDaemonLog(logPath));
    assert.equal(statSync(logPath).mode & 0o777, 0o600);
    chmodSync(logPath, 0o644);
    assert.throws(() => openDaemonLog(logPath), /mode 0600/);
    rmSync(logPath);
    writeFileSync(path.join(directory, "target.log"), "x");
    symlinkSync("target.log", logPath);
    assert.throws(() => openDaemonLog(logPath));
    mkdirSync(path.join(directory, "dir.log"));
    assert.throws(() => openDaemonLog(path.join(directory, "dir.log")));
    const scrubbed = scrubEnvironment({
      CAPSTAN_TOKEN: "t",
      CAPSTAN_SOCKET: "/s",
      PATH: "/bin",
      HOME: "/h",
    });
    assert.deepEqual(scrubbed, { PATH: "/bin", HOME: "/h" });
    void openSync;
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

import assert from "node:assert/strict";

// No test may reach a real Herdr session: the daemon and `cstan start` stay out of it.
process.env.CAPSTAN_LAUNCH = "off";
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
import { openSqlite } from "../src/controller/sqlite.js";
import {
  ControllerUnavailableError,
  callDaemon,
  ensureDaemon,
  logTail,
  openDaemonLog,
  stopDaemon,
  pingDaemon,
  scrubEnvironment,
} from "../src/client.js";
import {
  MAX_FRAME_BYTES,
  ROUTES,
  runDaemon,
  defaultVerificationHooks,
  removeStaleSocket,
  startDaemonServer,
  type CommandResponse,
  type DaemonServer,
  type LogEntry,
} from "../src/daemon.js";
import {
  call,
  close,
  ctx,
  harness,
  projectInfo,
  rawExchange,
} from "./harness.js";
import { ControllerCore } from "../src/controller/core.js";
import type { CapstanConfig } from "../src/config/capstan-config.js";
import type { Notifier } from "../src/notifier.js";
import { StubAdapter } from "./launcher-stubs.js";

const READ = Object.keys(ROUTES).filter(
  (name) => ROUTES[name]!.access === "read",
);
const AGENT = Object.keys(ROUTES).filter(
  (name) => ROUTES[name]!.access === "agent",
);
const OPERATOR = Object.keys(ROUTES).filter(
  (name) => ROUTES[name]!.access === "operator" && name !== "shutdown",
);
const ANY = Object.keys(ROUTES).filter(
  (name) => ROUTES[name]!.access === "any",
);

/** What a call with no arguments answers: a stub says so, a real command wants arguments. */
function bareAnswer(name: string): string {
  // Without a launcher the launch and restart commands say so first; spawn and release check their arguments first.
  if (["launch", "pm-restart", "op", "prompt"].includes(name))
    return "not_configured";
  return ROUTES[name]!.stub !== undefined
    ? "not_implemented"
    : "invalid_request";
}

function code(response: CommandResponse): string {
  return response.ok ? "ok" : response.code;
}

test("the operator credential is accepted for operator and read commands and refused for agent commands", async () => {
  const h = await harness();
  try {
    for (const name of READ)
      assert.equal(code(await call(h, h.owner, name)), "ok", name);
    for (const name of OPERATOR)
      assert.equal(code(await call(h, h.owner, name)), bareAnswer(name), name);
    for (const name of ANY)
      assert.equal(code(await call(h, h.owner, name)), bareAnswer(name), name);
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
      // wait is exercised in its own tests; here only the stubs and the
      // argument checks of the immediate commands are compared.
      for (const name of AGENT.filter((n) => n !== "wait"))
        assert.equal(
          code(await call(h, member.credential, name)),
          // Only workers report; a bare report from a worker fails its argument check first.
          name === "report" && member === h.pm
            ? "forbidden"
            : name === "finding"
              ? "forbidden"
              : name === "observe"
                ? member === h.pm
                  ? "invalid_request"
                  : "forbidden"
                : name === "request-review"
                  ? member === h.pm
                    ? "invalid_request"
                    : "forbidden"
                  : name === "review"
                    ? "forbidden"
                    : bareAnswer(name),
          name,
        );
      for (const name of ANY)
        assert.equal(
          code(await call(h, member.credential, name)),
          name === "inbox"
            ? "ok"
            : [
                  "spawn",
                  "release",
                  "replace",
                  "integrate",
                  "link",
                  "pause",
                  "resume",
                ].includes(name) && member === h.developer
              ? "forbidden"
              : bareAnswer(name),
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
    const db = openSqlite(path.join(h.stateDirectory, "controller.sqlite"), {
      readOnly: true,
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
    assert.equal(sendEntry.code, "unknown_recipient");
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

test("frames may arrive in pieces and the limit still applies; excess connections are dropped", async () => {
  const h = await harness();
  try {
    const request = JSON.stringify({
      v: 1,
      credential: h.owner,
      command: "ping",
      args: [],
      pad: "",
    });
    const build = (size: number): Buffer =>
      Buffer.from(
        request.replace(
          '"pad":""',
          `"pad":"${"p".repeat(size - Buffer.byteLength(request))}"`,
        ),
      );
    const pieces = (payload: Buffer, count: number): Buffer[] => {
      const size = Math.ceil(payload.length / count);
      return Array.from({ length: count }, (_, index) =>
        payload.subarray(index * size, (index + 1) * size),
      );
    };
    const send = (parts: Buffer[]): Promise<string> =>
      new Promise((resolve) => {
        const socket = net.createConnection(h.socketPath);
        let data = "";
        socket.on("data", (chunk) => (data += chunk.toString("utf8")));
        socket.on("close", () => resolve(data));
        socket.on("error", () => resolve(data));
        socket.on("connect", () => {
          void (async () => {
            for (const part of parts) {
              socket.write(part);
              await new Promise((done) => setTimeout(done, 20));
            }
          })();
        });
        socket.setTimeout(3000, () => socket.destroy());
      });
    const exact = Buffer.concat([build(MAX_FRAME_BYTES), Buffer.from("\n")]);
    assert.equal(
      (JSON.parse(await send(pieces(exact, 4))) as { ok: boolean }).ok,
      true,
    );
    const over = Buffer.concat([build(MAX_FRAME_BYTES + 1), Buffer.from("\n")]);
    assert.equal(await send(pieces(over, 4)), "");

    const idle: net.Socket[] = [];
    let dropped = 0;
    await new Promise<void>((resolve) => {
      let settled = 0;
      for (let index = 0; index < 90; index += 1) {
        const socket = net.createConnection(h.socketPath);
        idle.push(socket);
        socket.on("close", () => {
          dropped += 1;
        });
        socket.on("error", () => {});
        socket.on("connect", () => {
          settled += 1;
          if (settled === 90) setTimeout(resolve, 300);
        });
      }
      setTimeout(resolve, 2000);
    });
    assert.ok(
      dropped >= 20,
      `expected the connection cap to drop connections, dropped ${dropped}`,
    );
    for (const socket of idle) socket.destroy();
    await new Promise((resolve) => setTimeout(resolve, 200));
    assert.equal(code(await call(h, h.owner, "ping")), "ok");
  } finally {
    await close(h);
  }
});

test("a daemon that cannot be spawned is reported as a startup failure", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-spawn-"));
  try {
    await assert.rejects(
      ensureDaemon({
        socketPath: path.join(directory, "control.sock"),
        credential: "c".repeat(40),
        projectRoot: path.join(directory, "does-not-exist"),
        logPath: path.join(directory, "daemon.log"),
        cliPath: "/nonexistent/cli.js",
        env: {},
        timeoutMs: 3000,
      }),
      (error: unknown) =>
        error instanceof ControllerUnavailableError &&
        error.reason === "start_failed" &&
        /could not be started/.test(error.message),
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("a reply above the size limit becomes an error reply, and a long legacy action is logged capped", async () => {
  const stateDirectory = mkdtempSync(
    path.join(tmpdir(), "capstan-daemon-limit-"),
  );
  const info = projectInfo();
  const core = await ControllerCore.open({ stateDirectory, project: info });
  const log: LogEntry[] = [];
  const socketPath = path.join(stateDirectory, "control.sock");
  const server = await startDaemonServer({
    socketPath,
    core,
    log: (entry) => log.push(entry),
    onShutdown: () => {},
    maxResponseBytes: 300,
  });
  try {
    const status = await callDaemon(socketPath, info.ownerCredential, "status");
    assert.deepEqual(status, {
      kind: "response",
      response: {
        ok: false,
        code: "error",
        message: "response exceeds the size limit",
      },
    });
    const ping = await callDaemon(socketPath, info.ownerCredential, "ping");
    assert.equal((ping as { response: CommandResponse }).response.ok, true);
    const long = JSON.stringify({
      token: info.ownerCredential,
      action: "a".repeat(60_000),
    });
    await rawExchange(socketPath, Buffer.from(`${long}\n`));
    const legacy = log.find((entry) => entry.command.startsWith("control:"))!;
    assert.ok(legacy.command.length <= "control:".length + 32);
  } finally {
    await server.close();
    core.close();
    rmSync(stateDirectory, { recursive: true, force: true });
  }
});

test("a byte-order mark is rejected, a CRLF terminator is accepted, empty arguments are refused and the reply limit is per server", async () => {
  const h = await harness();
  try {
    const good = JSON.stringify({
      v: 1,
      credential: h.owner,
      command: "ping",
      args: [],
    });
    const bom = await rawExchange(
      h.socketPath,
      Buffer.concat([
        Buffer.from([0xef, 0xbb, 0xbf]),
        Buffer.from(`${good}\n`),
      ]),
    );
    assert.equal((JSON.parse(bom) as { code: string }).code, "invalid_request");
    const crlf = await rawExchange(h.socketPath, Buffer.from(`${good}\r\n`));
    assert.equal((JSON.parse(crlf) as { ok: boolean }).ok, true);
    const empty = JSON.stringify({
      v: 1,
      credential: h.owner,
      command: "ping",
      args: [""],
    });
    assert.equal(
      (
        JSON.parse(
          await rawExchange(h.socketPath, Buffer.from(`${empty}\n`)),
        ) as { code: string }
      ).code,
      "invalid_request",
    );
    const tiny = await startDaemonServer({
      socketPath: path.join(h.stateDirectory, "tiny.sock"),
      core: h.core,
      log: () => {},
      onShutdown: () => {},
      maxResponseBytes: 50,
    });
    try {
      const small = await callDaemon(
        path.join(h.stateDirectory, "tiny.sock"),
        h.owner,
        "status",
      );
      assert.equal((small as { response: { ok: boolean } }).response.ok, false);
      const normal = await call(h, h.owner, "status");
      assert.equal(
        normal.ok,
        true,
        "a limit set on one server does not leak into another",
      );
    } finally {
      await tiny.close();
    }
  } finally {
    await close(h);
  }
});

test("the log tail starts at this run, strips control characters and stays short", () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-tail-"));
  try {
    const logPath = path.join(directory, "daemon.log");
    const old = "OLD LINE\n";
    writeFileSync(
      logPath,
      `${old}first\nsecond\nthird \u001b[31mred\u001b[0m\u0007 bell\rX\nfourth\n`,
      { mode: 0o600 },
    );
    const tail = logTail(logPath, Buffer.byteLength(old));
    assert.ok(!tail.includes("OLD LINE"));
    assert.ok(
      !/[\u0000-\u0009\u000b-\u001f\u007f]/.test(tail),
      JSON.stringify(tail),
    );
    assert.match(
      tail,
      /^third .*red.* bell.X \| fourth$|^second \| third .*red.* bell.X \| fourth$/,
    );
    writeFileSync(logPath, `${"x".repeat(10_000)}\nlast\n`, { mode: 0o600 });
    assert.ok(logTail(logPath, 0).length <= 400);
    assert.match(logTail(logPath, 0), /last$/);
    writeFileSync(logPath, "", { mode: 0o600 });
    assert.equal(logTail(logPath, 0), "(the daemon wrote nothing)");
    assert.equal(
      logTail(path.join(directory, "missing.log"), 0),
      "(log unreadable)",
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test("stopDaemon returns only after the daemon process has exited, not when the listener closes", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "capstan-stop-"));
  const socketPath = path.join(directory, "control.sock");
  const script = `
    const net = require("node:net");
    const server = net.createServer((socket) => {
      socket.once("data", (chunk) => {
        const request = JSON.parse(chunk.toString("utf8"));
        if (request.command === "shutdown") {
          socket.end(JSON.stringify({ ok: true, result: { stopping: true } }) + "\\n");
          server.close();
          setTimeout(() => process.exit(0), 1200);
        } else {
          socket.end(JSON.stringify({ ok: true, result: { pong: true, pid: process.pid } }) + "\\n");
        }
      });
    });
    server.listen(process.argv[1], () => console.log("up"));
  `;
  const child = spawn(process.execPath, ["-e", script, socketPath], {
    stdio: ["ignore", "pipe", "ignore"],
  });
  try {
    await new Promise<void>((resolve) =>
      child.stdout.once("data", () => resolve()),
    );
    const began = Date.now();
    assert.equal(await stopDaemon(socketPath, "c".repeat(40), 8000), "stopped");
    assert.ok(Date.now() - began >= 1000, "returned before the process exited");
    assert.throws(() => process.kill(child.pid!, 0), /ESRCH/);
  } finally {
    child.kill("SIGKILL");
    rmSync(directory, { recursive: true, force: true });
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
        error: "invalid control request",
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

test("pingDaemon tells a running daemon, a refusal, no daemon, a reply that is not a daemon frame and a silent one apart", async () => {
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
    const notDaemon = await serve("not-daemon.sock", (socket) => {
      socket.once("data", () => socket.end('{"error":"unauthorized"}\n'));
    });
    assert.equal((await pingDaemon(notDaemon, h.owner)).outcome, "unreachable");
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

test("the daemon passes a process probe to the driver, so a working agent's pane is sampled", async () => {
  const root = mkdtempSync(path.join(tmpdir(), "capstan-daemon-probe-"));
  const stateDirectory = path.join(root, "state");
  const info = projectInfo();
  const seed = await ControllerCore.open({ stateDirectory, project: info });
  seed.syncRoleDefinitions(ctx(seed, info.ownerCredential), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
  ]);
  seed.close();
  const pm = Object.defineProperty(
    {
      name: "pm",
      kind: "PM",
      host: "claude",
      model: null,
      permissionMode: "default",
      allow: [],
      deny: [],
      hooks: "off",
      prompt: { source: "none", path: null, hash: null },
      configHash: "a".repeat(64),
    },
    "promptText",
    { value: null, enumerable: false },
  );
  const capstan = {
    schemaVersion: 1,
    projectName: null,
    herdrSession: "unused",
    notifications: { herdr: false, fallback: true },
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
    limits: { maxWorkers: 3 },
    ledger: { keepMigrationBackups: 3 },
    layout: {
      spawn: "tab",
      split: "auto",
      minPaneColumns: 60,
      minPaneRows: 12,
    },
    env: { pass: [] },
    hosts: [
      {
        name: "claude",
        kind: "claude",
        command: "claude",
        shellCommandTimeoutSeconds: 120,
        waitTimeoutSeconds: 45,
      },
    ],
    roles: [pm],
  } as unknown as CapstanConfig;
  const notifier: Notifier = {
    send: async () => [{ channel: "fallback", ok: true }],
    write: () => undefined,
  };
  const socket = path.join(stateDirectory, "control.sock");
  const adapter = new StubAdapter();
  adapter.observation = "working";
  const sampled: string[] = [];
  let ready!: () => void;
  const up = new Promise<void>((resolve) => {
    ready = resolve;
  });
  const done = runDaemon({
    stateDirectory,
    project: info,
    workspaceRoot: root,
    log: () => undefined,
    announce: (event) => {
      if (event.event === "ready") ready();
    },
    capstan,
    adapter,
    notifier,
    cliPath: "/opt/capstan/cli.js",
    tickMs: 500,
    processProbe: {
      sample: async (paneId) => {
        sampled.push(paneId);
        return { processes: [] };
      },
    },
  });
  try {
    await up;
    const launched = await callDaemon(
      socket,
      info.ownerCredential,
      "launch",
      [],
      30_000,
    );
    assert.equal(launched.kind, "response");
    const deadline = Date.now() + 5000;
    while (sampled.length === 0 && Date.now() < deadline)
      await new Promise((resolve) => setTimeout(resolve, 50));
    assert.ok(sampled.length > 0, "the driver sampled through the probe");
  } finally {
    await callDaemon(socket, info.ownerCredential, "shutdown", [], 30_000);
    await done;
    rmSync(root, { recursive: true, force: true });
  }
});

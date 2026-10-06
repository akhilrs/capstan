import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readdirSync,
  realpathSync,
  rmSync,
} from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { socketVerdict } from "../src/cli.js";

const cli = path.resolve(import.meta.dirname, "..", "src", "cli.js");

interface Run {
  status: number | null;
  stdout: string;
  stderr: string;
}

function run(
  cwd: string,
  env: NodeJS.ProcessEnv,
  ...args: string[]
): Promise<Run> {
  return new Promise((resolve, reject) => {
    const base = { ...process.env };
    delete base.CAPSTAN_TOKEN;
    delete base.CAPSTAN_SOCKET;
    delete base.CAPSTAN_ALLOW_FOREIGN_SOCKET;
    const child = spawn(process.execPath, [cli, ...args], {
      cwd,
      env: { ...base, ...env },
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += String(chunk)));
    child.stderr.on("data", (chunk) => (stderr += String(chunk)));
    child.on("error", reject);
    child.on("close", (status) => resolve({ status, stdout, stderr }));
  });
}

async function fixture(): Promise<{
  a: string;
  b: string;
  socketB: string;
  connections: () => number;
  cleanup: () => Promise<void>;
}> {
  const top = realpathSync(mkdtempSync(path.join(os.tmpdir(), "cgd-")));
  const a = path.join(top, "a");
  const b = path.join(top, "b");
  mkdirSync(path.join(a, ".capstan", "state"), { recursive: true });
  mkdirSync(path.join(b, ".capstan", "state"), { recursive: true });
  const socketB = path.join(b, ".capstan", "state", "control.sock");
  let count = 0;
  const server = net.createServer((socket) => {
    count += 1;
    socket.on("error", () => {});
    socket.on("data", () => {
      socket.end(`${JSON.stringify({ ok: true, result: { pong: true } })}\n`);
    });
  });
  await new Promise<void>((resolve) => server.listen(socketB, resolve));
  return {
    a,
    b,
    socketB,
    connections: () => count,
    cleanup: async () => {
      await new Promise((resolve) => server.close(resolve));
      rmSync(top, { recursive: true, force: true });
    },
  };
}

test("socketVerdict is pure and classifies the socket", async () => {
  const f = await fixture();
  try {
    const own = path.join(f.a, ".capstan", "state", "control.sock");
    assert.equal(socketVerdict(f.a, {}).kind, "none");
    assert.equal(socketVerdict(f.a, { CAPSTAN_SOCKET: own }).kind, "match");
    const foreign = socketVerdict(f.a, { CAPSTAN_SOCKET: f.socketB });
    assert.equal(foreign.kind, "foreign");
    assert.equal(foreign.projectRoot, f.a);
    assert.equal(foreign.expectedSocket, own);
    const bare = mkdtempSync(path.join(os.tmpdir(), "cgd-bare-"));
    try {
      assert.equal(
        socketVerdict(bare, { CAPSTAN_SOCKET: f.socketB }).kind,
        bare.startsWith(f.a) ? "foreign" : "none",
      );
    } finally {
      rmSync(bare, { recursive: true, force: true });
    }
  } finally {
    await f.cleanup();
  }
});

test("a foreign CAPSTAN_SOCKET is refused before any request", async () => {
  const f = await fixture();
  try {
    const before = readdirSync(path.join(f.a, ".capstan")).join();
    const env = { CAPSTAN_TOKEN: "tok", CAPSTAN_SOCKET: f.socketB };
    for (const args of [
      ["ping"],
      ["status"],
      ["status", "--json"],
      ["inbox"],
      ["wait"],
      ["send", "@pm", "hello"],
      ["dash"],
    ]) {
      const result = await run(f.a, env, ...args);
      assert.notEqual(result.status, 0, args.join(" "));
      {
        assert.equal(result.status, 2, `${args.join(" ")}: ${result.stderr}`);
        assert.ok(result.stderr.includes(f.socketB), result.stderr);
        assert.ok(
          result.stderr.includes(
            path.join(f.a, ".capstan", "state", "control.sock"),
          ),
          result.stderr,
        );
        assert.match(result.stderr, /CAPSTAN_ALLOW_FOREIGN_SOCKET=1/);
      }
    }
    assert.equal(f.connections(), 0);
    assert.equal(readdirSync(path.join(f.a, ".capstan")).join(), before);
  } finally {
    await f.cleanup();
  }
});

test("inbox --hook on a foreign socket is silent and makes no request", async () => {
  const f = await fixture();
  try {
    for (const allow of [{}, { CAPSTAN_ALLOW_FOREIGN_SOCKET: "1" }]) {
      const result = await run(
        f.a,
        {
          CAPSTAN_TOKEN: "tok",
          CAPSTAN_SOCKET: f.socketB,
          CAPSTAN_AGENT_ID: "dev-1",
          ...allow,
        },
        "inbox",
        "--hook",
      );
      assert.equal(result.status, 0);
      assert.equal(result.stdout, "");
      assert.equal(result.stderr, "");
    }
    assert.equal(f.connections(), 0);
  } finally {
    await f.cleanup();
  }
});

test("CAPSTAN_ALLOW_FOREIGN_SOCKET=1 warns once and proceeds", async () => {
  const f = await fixture();
  try {
    const env = {
      CAPSTAN_TOKEN: "tok",
      CAPSTAN_SOCKET: f.socketB,
      CAPSTAN_ALLOW_FOREIGN_SOCKET: "1",
    };
    for (const args of [
      ["ping"],
      ["status"],
      ["inbox"],
      ["send", "@pm", "hi"],
    ]) {
      const before = f.connections();
      const result = await run(f.a, env, ...args);
      assert.notEqual(result.status, 2, `${args.join(" ")}: ${result.stderr}`);
      assert.equal(
        result.stderr.split("\n").filter((line) => /^warning: /.test(line))
          .length,
        1,
        result.stderr,
      );
      assert.equal(f.connections(), before + 1, args.join(" "));
    }
  } finally {
    await f.cleanup();
  }
});

test("a matching socket and a directory without .capstan behave as before", async () => {
  const f = await fixture();
  const bare = mkdtempSync(path.join(os.tmpdir(), "cgd-bare-"));
  try {
    const own = path.join(f.b, ".capstan", "state", "control.sock");
    const match = await run(
      f.b,
      { CAPSTAN_TOKEN: "tok", CAPSTAN_SOCKET: own },
      "ping",
    );
    assert.equal(match.status, 0, match.stderr);
    assert.doesNotMatch(match.stderr, /warning/);
    const before = f.connections();
    const none = await run(
      bare,
      { CAPSTAN_TOKEN: "tok", CAPSTAN_SOCKET: own },
      "ping",
    );
    assert.equal(none.status, 0, none.stderr);
    assert.equal(f.connections(), before + 1);
  } finally {
    rmSync(bare, { recursive: true, force: true });
    await f.cleanup();
  }
});

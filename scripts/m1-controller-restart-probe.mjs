import { spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, fsyncSync, mkdirSync, openSync, readFileSync, rmSync, writeSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

const SELF = fileURLToPath(import.meta.url);
const COMMAND_ID = "pm4-controller-crash-001";

function appendDurably(file, entry) {
  const fd = openSync(file, "a", 0o600);
  try {
    writeSync(fd, `${JSON.stringify({ at: new Date().toISOString(), ...entry })}\n`);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
}

function connect(container) {
  const proc = spawn("docker", ["attach", "--sig-proxy=false", container], { stdio: ["pipe", "pipe", "ignore"] });
  const lines = readline.createInterface({ input: proc.stdout });
  const frames = [];
  const waiters = [];
  let closed = false;
  proc.on("close", (code) => {
    closed = true;
    for (const waiter of waiters.splice(0)) waiter.reject(new Error(`RPC attach closed (${code})`));
  });
  lines.on("line", (line) => {
    let frame;
    try { frame = JSON.parse(line); } catch { return; }
    const index = waiters.findIndex((waiter) => waiter.predicate(frame));
    if (index >= 0) waiters.splice(index, 1)[0].resolve(frame);
    else frames.push(frame);
  });
  const waitFrame = (predicate, timeoutMs = 45_000) => {
    const index = frames.findIndex(predicate);
    if (index >= 0) return Promise.resolve(frames.splice(index, 1)[0]);
    if (closed) return Promise.reject(new Error("RPC attach is closed"));
    return new Promise((resolve, reject) => {
      const waiter = { predicate, resolve, reject };
      const timer = setTimeout(() => {
        const i = waiters.indexOf(waiter);
        if (i >= 0) waiters.splice(i, 1);
        reject(new Error("RPC frame timeout"));
      }, timeoutMs);
      waiter.resolve = (value) => { clearTimeout(timer); resolve(value); };
      waiter.reject = (error) => { clearTimeout(timer); reject(error); };
      waiters.push(waiter);
    });
  };
  return { proc, lines, waitFrame, send: (command) => proc.stdin.write(`${JSON.stringify(command)}\n`) };
}

async function controller(mode) {
  const container = process.env.M1_CRASH_CONTAINER;
  const journalPath = process.env.M1_CRASH_JOURNAL;
  if (!container || !journalPath) throw new Error("Crash controller environment is incomplete");
  const rpc = connect(container);
  try {
    const stateId = `controller-${mode}-ready`;
    rpc.send({ id: stateId, type: "get_state" });
    const state = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === stateId);
    if (!state.success) throw new Error("OMP RPC transport readiness check failed");
    if (mode === "dispatch") {
      appendDurably(journalPath, { event: "dispatch", commandId: COMMAND_ID, rpcId: COMMAND_ID, type: "prompt" });
      rpc.send({ id: COMMAND_ID, type: "prompt", message: `Command identity ${COMMAND_ID}. Reply exactly M1_CRASH_ACK. Do not run tools.` });
      const response = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === COMMAND_ID);
      if (!response.success) throw new Error("OMP rejected the crash-boundary prompt");
      rpc.lines.pause();
      process.stdout.write("DISPATCHED\n");
      await new Promise(() => {});
    }
    if (mode !== "recover") throw new Error(`Unknown controller mode ${mode}`);
    const records = readFileSync(journalPath, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    const dispatches = records.filter((entry) => entry.event === "dispatch" && entry.commandId === COMMAND_ID);
    const prematureReceipts = records.filter((entry) => entry.commandId === COMMAND_ID && entry.event === "receipt");
    if (dispatches.length !== 1 || prematureReceipts.length !== 0) throw new Error("Restart did not find exactly one pending durable outbox entry");

    const deadline = Date.now() + 90_000;
    let matchingEntries = 0;
    let reply;
    while (Date.now() < deadline) {
      const id = `reconcile-${Date.now()}`;
      rpc.send({ id, type: "get_entries" });
      const response = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === id);
      if (!response.success) throw new Error("OMP history reconciliation failed");
      const entries = response.data?.entries ?? [];
      matchingEntries = JSON.stringify(entries).split(COMMAND_ID).length - 1;
      const readbackId = `${id}-last`;
      rpc.send({ id: readbackId, type: "get_last_assistant_text" });
      const readback = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === readbackId);
      if (!readback.success) throw new Error("OMP assistant readback failed");
      reply = readback.data?.text;
      if (matchingEntries === 1 && reply === "M1_CRASH_ACK") break;
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
    if (matchingEntries !== 1 || reply !== "M1_CRASH_ACK") {
      throw new Error(`Restart reconciliation mismatch (command occurrences=${matchingEntries}, exact reply=${reply === "M1_CRASH_ACK"})`);
    }
    appendDurably(journalPath, { event: "receipt", commandId: COMMAND_ID, reconciled: true, resent: false, matchingHistoryEntries: matchingEntries });
    process.stdout.write(JSON.stringify({ recovered: true, matchingHistoryEntries: matchingEntries, exactReply: true, resent: false }) + "\n");
  } finally {
    rpc.proc.kill("SIGKILL");
  }
}

function runChild(mode, container, journalPath) {
  const child = spawn(process.execPath, [SELF, mode], {
    detached: true,
    env: { ...process.env, M1_CRASH_CONTAINER: container, M1_CRASH_JOURNAL: journalPath },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.diagnostic = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { child.diagnostic += chunk; });
  return child;
}

export async function runControllerRestartProbe({ image, omp, addon, model, token, root }) {
  const dir = path.join(root, "controller-crash");
  const home = path.join(dir, "home");
  const workspace = path.join(dir, "workspace");
  mkdirSync(home, { recursive: true, mode: 0o700 });
  mkdirSync(workspace, { recursive: true, mode: 0o700 });
  const journalPath = path.join(dir, "rpc-journal.jsonl");
  const name = `capstan-m1-restart-${process.pid}`;
  const runtimeArgv = [
    "/usr/local/bin/omp", "--mode", "rpc", "--model", model,
    "--profile", "m1-crash-restart", "--cwd", "/workspace",
    "--session-dir", "/home/worker/sessions/crash-restart",
    "--system-prompt", "You are the PM seat. Follow only the current user request.",
    "--no-ui", "--no-extensions", "--no-skills", "--no-rules", "--no-tools",
  ];
  const started = spawnSync("docker", [
    "run", "--detach", "--interactive", "--name", name, "--label", "capstan.m1.probe=true",
    "--network", "bridge", "--user", `${process.getuid()}:${process.getgid()}`,
    "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
    "--mount", `type=bind,src=${home},dst=/home/worker`,
    "--mount", `type=bind,src=${workspace},dst=/workspace`,
    "--mount", `type=bind,src=${omp},dst=/usr/local/bin/omp,readonly`,
    "--mount", `type=bind,src=${addon},dst=/usr/local/bin/pi_natives.linux-x64-baseline.node,readonly`,
    "--tmpfs", "/tmp:rw,nosuid,nodev", "--env", "HOME=/home/worker", "--env", "OPENAI_CODEX_OAUTH_TOKEN",
    image, ...runtimeArgv,
  ], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], env: { ...process.env, OPENAI_CODEX_OAUTH_TOKEN: token } });
  if (started.status !== 0 || !started.stdout.trim()) throw new Error("Could not start persistent crash-recovery OMP worker");

  let firstController;
  let recoveryController;
  try {
    firstController = runChild("dispatch", name, journalPath);
    const dispatched = await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error("Initial controller did not reach the dispatch boundary")), 60_000);
      const lines = readline.createInterface({ input: firstController.stdout });
      lines.once("line", (line) => {
        clearTimeout(timeout);
        if (line !== "DISPATCHED") reject(new Error("Initial controller failed before durable dispatch"));
        else resolve(true);
      });
      firstController.once("close", (code) => {
        clearTimeout(timeout);
        reject(new Error(`Initial controller exited before dispatch (${code})`));
      });
    });
    if (!dispatched) throw new Error("Initial controller did not dispatch");
    process.kill(-firstController.pid, "SIGKILL");
    await new Promise((resolve) => firstController.once("close", resolve));
    const state = spawnSync("docker", ["inspect", "--format", "{{.State.Status}}", name], { encoding: "utf8" });
    if (state.status !== 0 || state.stdout.trim() !== "running") throw new Error("OMP worker did not survive controller SIGKILL");

    recoveryController = runChild("recover", name, journalPath);
    const result = await new Promise((resolve, reject) => {
      let output = "";
      const timeout = setTimeout(() => reject(new Error("Replacement controller reconciliation timed out")), 120_000);
      recoveryController.stdout.setEncoding("utf8");
      recoveryController.stdout.on("data", (chunk) => { output += chunk; });
      recoveryController.once("close", (code) => {
        clearTimeout(timeout);
        if (code !== 0) reject(new Error(`Replacement controller failed (${code}): ${recoveryController.diagnostic.trim()}`));
        else {
          try { resolve(JSON.parse(output.trim())); } catch { reject(new Error("Replacement controller returned invalid result")); }
        }
      });
    });
    const records = readFileSync(journalPath, "utf8").trim().split("\n").map((line) => JSON.parse(line));
    if (records.filter((entry) => entry.event === "dispatch" && entry.commandId === COMMAND_ID).length !== 1
      || records.filter((entry) => entry.event === "receipt" && entry.commandId === COMMAND_ID).length !== 1) {
      throw new Error("Crash journal did not retain one dispatch and one reconciled receipt");
    }
    return { controllerKilled: true, workerSurvived: true, ...result };
  } finally {
    if (firstController?.exitCode === null) {
      try { process.kill(-firstController.pid, "SIGKILL"); } catch {}
    }
    if (recoveryController?.exitCode === null) {
      try { process.kill(-recoveryController.pid, "SIGKILL"); } catch {}
    }
    spawnSync("docker", ["rm", "-f", name], { stdio: "ignore" });
  }
}

if (process.argv[1] === SELF) {
  controller(process.argv[2]).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}

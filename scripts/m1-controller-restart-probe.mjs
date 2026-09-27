import { spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, fsyncSync, ftruncateSync, mkdirSync, openSync, readFileSync, rmSync, writeSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

const SELF = fileURLToPath(import.meta.url);
const COMMAND_ID = "pm4-controller-crash-001";
const PROMPT = `Command identity ${COMMAND_ID}. Reply exactly M1_CRASH_ACK. Do not run tools.`;

function appendDurably(file, entry) {
  const fd = openSync(file, "a", 0o600);
  try {
    const bytes = Buffer.from(`${JSON.stringify({ at: new Date().toISOString(), ...entry })}\n`);
    for (let offset = 0; offset < bytes.length;) {
      const written = writeSync(fd, bytes, offset, bytes.length - offset);
      if (written <= 0) throw new Error("Crash journal write made no progress");
      offset += written;
    }
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
}

function readJournal(file) {
  const bytes = readFileSync(file);
  const committedEnd = bytes.lastIndexOf(10) + 1;
  if (committedEnd !== bytes.length) {
    const fd = openSync(file, "r+");
    try {
      ftruncateSync(fd, committedEnd);
      fsyncSync(fd);
    } finally {
      closeSync(fd);
    }
  }
  if (committedEnd === 0) return [];
  return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes.subarray(0, committedEnd))
    .slice(0, -1).split("\n").map((line) => JSON.parse(line));
}

function connect(container) {
  const proc = spawn("docker", ["attach", "--sig-proxy=false", container], { stdio: ["pipe", "pipe", "ignore"] });
  const frames = [];
  const waiters = [];
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  let pending = "";
  let closed = false;
  let wireError;
  const failWire = (error) => {
    wireError ??= error;
    for (const waiter of waiters.splice(0)) waiter.reject(wireError);
    proc.kill();
  };
  proc.on("error", failWire);
  proc.stdin.on("error", failWire);
  proc.stdout.on("error", failWire);
  proc.on("close", (code) => {
    try {
      pending += decoder.decode();
      if (pending) throw new Error("Unterminated RPC frame");
    } catch (error) { wireError ??= error; }
    closed = true;
    for (const waiter of waiters.splice(0)) waiter.reject(wireError ?? new Error(`RPC attach closed (${code})`));
  });
  proc.stdout.on("data", (chunk) => {
    try {
      pending += decoder.decode(chunk, { stream: true });
      let end;
      while ((end = pending.indexOf("\n")) !== -1) {
        const line = pending.slice(0, end);
        pending = pending.slice(end + 1);
        if (Buffer.byteLength(line, "utf8") > 1_048_576) throw new Error("RPC frame exceeds 1 MiB");
        const frame = JSON.parse(line);
        const index = waiters.findIndex((waiter) => waiter.predicate(frame));
        if (index >= 0) waiters.splice(index, 1)[0].resolve(frame);
        else {
          if (frames.length >= 64) throw new Error("Too many queued RPC frames");
          frames.push(frame);
        }
      }
      if (Buffer.byteLength(pending, "utf8") > 1_048_576) throw new Error("RPC frame exceeds 1 MiB");
    } catch (error) { failWire(error); }
  });
  const waitFrame = (predicate, timeoutMs = 45_000) => {
    if (wireError) return Promise.reject(wireError);
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
  return { proc, waitFrame, send: (command) => proc.stdin.write(`${JSON.stringify(command)}\n`) };
}

async function controller(mode) {
  const container = process.env.M1_CRASH_CONTAINER;
  const journalPath = process.env.M1_CRASH_JOURNAL;
  if (!container || !journalPath) throw new Error("Crash controller environment is incomplete");
  if (mode !== "dispatch" && mode !== "recover") throw new Error(`Unknown controller mode ${mode}`);
  if (mode === "recover") {
    const records = readJournal(journalPath);
    const dispatches = records.filter((entry) => entry.event === "dispatch" && entry.commandId === COMMAND_ID);
    const receipts = records.filter((entry) => entry.commandId === COMMAND_ID && entry.event === "receipt");
    if (dispatches.length !== 1 || receipts.length > 1
      || dispatches[0].rpcId !== COMMAND_ID || dispatches[0].type !== "prompt") {
      throw new Error("Restart did not find one valid durable prompt dispatch and at most one receipt");
    }
    if (receipts.length === 1) {
      if (records.indexOf(receipts[0]) <= records.indexOf(dispatches[0])
        || receipts[0].rpcId !== dispatches[0].rpcId || receipts[0].type !== dispatches[0].type
        || receipts[0].reconciled !== true || receipts[0].matchingHistoryEntries !== 1 || receipts[0].resent !== false) {
        throw new Error("Existing recovery receipt is invalid");
      }
      process.stdout.write(JSON.stringify({ recovered: true, matchingHistoryEntries: 1, exactReply: true, resent: false, alreadyReconciled: true }) + "\n");
      return;
    }
  }
  const rpc = connect(container);
  try {
    const stateId = `controller-${mode}-ready`;
    rpc.send({ id: stateId, type: "get_state" });
    const state = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === stateId);
    if (!state.success) throw new Error("OMP RPC transport readiness check failed");
    if (mode === "dispatch") {
      appendDurably(journalPath, { event: "dispatch", commandId: COMMAND_ID, rpcId: COMMAND_ID, type: "prompt" });
      rpc.send({ id: COMMAND_ID, type: "prompt", message: PROMPT });
      const response = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === COMMAND_ID);
      if (!response.success) throw new Error("OMP rejected the crash-boundary prompt");
      rpc.proc.stdout.pause();
      process.stdout.write("DISPATCHED\n");
      await new Promise(() => {});
    }

    const deadline = Date.now() + 90_000;
    let matchingEntries = 0;
    let linkedReplies = 0;
    let reply;
    while (Date.now() < deadline) {
      const id = `reconcile-${Date.now()}`;
      rpc.send({ id, type: "get_entries" });
      const response = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === id);
      if (!response.success) throw new Error("OMP history reconciliation failed");
      const entries = response.data?.entries;
      if (!Array.isArray(entries)) throw new Error("OMP returned no crash-recovery history");
      const matchingUsers = entries.filter((entry) => {
        const contents = entry.message?.content;
        return entry.type === "message" && entry.message?.role === "user" && Array.isArray(contents)
          && contents.length === 1 && contents[0].type === "text" && contents[0].text === PROMPT;
      });
      matchingEntries = matchingUsers.length;
      linkedReplies = matchingEntries === 1 ? entries.filter((entry) => {
        const contents = entry.message?.content;
        return entry.type === "message" && entry.parentId === matchingUsers[0].id
          && entry.message?.role === "assistant" && Array.isArray(contents) && contents.length === 1
          && contents[0].type === "text" && contents[0].text === "M1_CRASH_ACK";
      }).length : 0;
      const readbackId = `${id}-last`;
      rpc.send({ id: readbackId, type: "get_last_assistant_text" });
      const readback = await rpc.waitFrame((frame) => frame.type === "response" && frame.id === readbackId);
      if (!readback.success) throw new Error("OMP assistant readback failed");
      reply = readback.data?.text;
      if (matchingEntries === 1 && linkedReplies === 1 && reply === "M1_CRASH_ACK") break;
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
    if (matchingEntries !== 1 || linkedReplies !== 1 || reply !== "M1_CRASH_ACK") {
      throw new Error(`Restart reconciliation mismatch (user entries=${matchingEntries}, linked replies=${linkedReplies}, exact last reply=${reply === "M1_CRASH_ACK"})`);
    }
    appendDurably(journalPath, { event: "receipt", commandId: COMMAND_ID, rpcId: COMMAND_ID, type: "prompt", reconciled: true, resent: false, matchingHistoryEntries: matchingEntries });
    process.stdout.write(JSON.stringify({ recovered: true, matchingHistoryEntries: matchingEntries, exactReply: true, resent: false }) + "\n");
  } finally {
    rpc.proc.kill("SIGKILL");
  }
}

function runChild(mode, container, journalPath) {
  const child = spawn("flock", ["-x", "-w", "110", journalPath, process.execPath, SELF, mode], {
    detached: true,
    env: { ...process.env, M1_CRASH_CONTAINER: container, M1_CRASH_JOURNAL: journalPath },
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.diagnostic = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { child.diagnostic += chunk; });
  child.on("error", (error) => { child.diagnostic += error.message; });
  return child;
}

function awaitRecovery(child) {
  return new Promise((resolve, reject) => {
    let output = "";
    const timeout = setTimeout(() => reject(new Error("Replacement controller reconciliation timed out")), 120_000);
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (chunk) => { output += chunk; });
    child.once("close", (code) => {
      clearTimeout(timeout);
      if (code !== 0) reject(new Error(`Replacement controller failed (${code}): ${child.diagnostic.trim()}`));
      else {
        try { resolve(JSON.parse(output.trim())); } catch { reject(new Error("Replacement controller returned invalid result")); }
      }
    });
  });
}

export async function runControllerRestartProbe({ image, omp, addon, model, token, root }) {
  const dir = path.join(root, "controller-crash");
  const home = path.join(dir, "home");
  const workspace = path.join(dir, "workspace");
  mkdirSync(home, { recursive: true, mode: 0o700 });
  mkdirSync(workspace, { recursive: true, mode: 0o700 });
  const journalPath = path.join(dir, "rpc-journal.jsonl");
  const journalFd = openSync(journalPath, "a", 0o600);
  fsyncSync(journalFd);
  closeSync(journalFd);
  const dirFd = openSync(dir, "r");
  fsyncSync(dirFd);
  closeSync(dirFd);
  const name = `capstan-m1-restart-${process.pid}`;
  const runtimeArgv = [
    "/usr/local/bin/omp", "--mode", "rpc", "--model", model,
    "--profile", "m1-crash-restart", "--cwd", "/workspace",
    "--session-dir", "/home/worker/sessions/crash-restart",
    "--system-prompt", "You are the PM seat. Follow only the current user request.",
    "--no-ui", "--no-extensions", "--no-skills", "--no-rules", "--no-tools",
  ];
  let firstController;
  let recoveryController;
  let competingController;
  try {
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

    const tornFd = openSync(journalPath, "a");
    try {
      writeSync(tornFd, '{"event":"receipt"');
      fsyncSync(tornFd);
    } finally {
      closeSync(tornFd);
    }
    recoveryController = runChild("recover", name, journalPath);
    competingController = runChild("recover", name, journalPath);
    const [result, concurrent] = await Promise.all([awaitRecovery(recoveryController), awaitRecovery(competingController)]);
    if (Number(result.alreadyReconciled === true) + Number(concurrent.alreadyReconciled === true) !== 1) {
      throw new Error("Concurrent recovery did not serialize into one receipt writer and one reader");
    }
    const stoppedWorker = spawnSync("docker", ["kill", "--signal", "KILL", name], { stdio: "ignore" });
    if (stoppedWorker.status !== 0) throw new Error("Could not stop worker before repeated recovery");
    recoveryController = runChild("recover", name, journalPath);
    const repeated = await awaitRecovery(recoveryController);
    if (!repeated.alreadyReconciled || !repeated.recovered || !repeated.exactReply || repeated.resent) {
      throw new Error("Second recovery did not reuse the durable receipt");
    }
    const journalText = readFileSync(journalPath, "utf8");
    if (!journalText.endsWith("\n")) throw new Error("Torn journal tail remained after recovery");
    const records = readJournal(journalPath);
    if (records.filter((entry) => entry.event === "dispatch" && entry.commandId === COMMAND_ID).length !== 1
      || records.filter((entry) => entry.event === "receipt" && entry.commandId === COMMAND_ID).length !== 1) {
      throw new Error("Crash journal did not retain one dispatch and one reconciled receipt");
    }
    return { controllerKilled: true, workerSurvived: true, tornTailRecovered: true, idempotentRecovery: true, ...result };
  } finally {
    if (firstController?.exitCode === null) {
      try { process.kill(-firstController.pid, "SIGKILL"); } catch {}
    }
    if (recoveryController?.exitCode === null) {
      try { process.kill(-recoveryController.pid, "SIGKILL"); } catch {}
    }
    if (competingController?.exitCode === null) {
      try { process.kill(-competingController.pid, "SIGKILL"); } catch {}
    }
    const removed = spawnSync("docker", ["rm", "-f", name], { stdio: "ignore" });
    if (removed.status !== 0) {
      const remaining = spawnSync("docker", ["ps", "--all", "--filter", `name=^/${name}$`, "--format", "{{.ID}}"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
      if (remaining.status !== 0 || remaining.stdout.trim()) throw new Error(`Could not prove crash-recovery worker ${name} was removed`);
    }
  }
}

if (process.argv[1] === SELF) {
  controller(process.argv[2]).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}

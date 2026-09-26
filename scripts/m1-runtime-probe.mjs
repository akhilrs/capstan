#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import {
  copyFileSync,
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  rmSync,
  statSync,
  writeSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { runControllerRestartProbe } from "./m1-controller-restart-probe.mjs";

function preserveJournals() {
  let parent = os.homedir();
  for (const segment of [".local", "state", "capstan", "m1-probe"]) {
    const child = path.join(parent, segment);
    if (!existsSync(child)) {
      mkdirSync(child, { mode: 0o700 });
      const fd = openSync(parent, "r");
      fsyncSync(fd);
      closeSync(fd);
    }
    parent = child;
  }
  const stateRoot = parent;
  const evidenceDir = mkdtempSync(path.join(stateRoot, "run-"));
  for (const [source, name] of [
    [journalPath, "controller.jsonl"],
    [path.join(root, "controller-crash", "rpc-journal.jsonl"), "controller-crash.jsonl"],
  ]) {
    if (!existsSync(source)) continue;
    const destination = path.join(evidenceDir, name);
    copyFileSync(source, destination);
    const fd = openSync(destination, "r");
    fsyncSync(fd);
    closeSync(fd);
  }
  const dirFd = openSync(evidenceDir, "r");
  fsyncSync(dirFd);
  closeSync(dirFd);
  const parentFd = openSync(stateRoot, "r");
  fsyncSync(parentFd);
  closeSync(parentFd);
  return evidenceDir;
}

const IMAGE = "ubuntu@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3";
const VERSION = "18.3.1";
const HERDR_CLIENT_VERSION = "0.9.1";
const HERDR_SERVER_VERSION = "0.9.0";
const MODEL = process.env.M1_OMP_MODEL ?? "openai-codex/gpt-6-sol";
const TOKEN_PROVIDER = process.env.M1_TOKEN_PROVIDER ?? "openai-codex";
const OMP = process.env.M1_OMP_BIN ?? findBinary("omp");
const ADDON = process.env.M1_OMP_NATIVE_ADDON ?? path.join(os.homedir(), ".omp", "natives", VERSION, "pi_natives.linux-x64-baseline.node");
const tokenResult = spawnSync(OMP, ["token", TOKEN_PROVIDER, "--raw"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
if (tokenResult.status !== 0 || !tokenResult.stdout.trim()) fail(`No OMP token available for provider ${TOKEN_PROVIDER}`);
if (!existsSync(OMP) || !existsSync(ADDON)) fail("Pinned OMP binary/native addon missing; set M1_OMP_BIN and M1_OMP_NATIVE_ADDON");
const versionResult = spawnSync(OMP, ["--help"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
if (!versionResult.stdout.startsWith(`omp v${VERSION}\n`)) fail(`Expected OMP ${VERSION}`);
const herdrClientVersion = spawnSync("herdr", ["--version"], { encoding: "utf8", env: { ...process.env, HERDR_ENV: "1" } });
if (herdrClientVersion.status !== 0 || herdrClientVersion.stdout.trim() !== `herdr ${HERDR_CLIENT_VERSION}`) fail(`Expected Herdr client ${HERDR_CLIENT_VERSION}`);
const herdrServerStatus = spawnSync("herdr", ["status", "server"], { encoding: "utf8", env: { ...process.env, HERDR_ENV: "1" } });
const herdrServerVersion = herdrServerStatus.stdout.match(/^version:\s*(\S+)\s*$/m)?.[1];
if (herdrServerStatus.status !== 0 || herdrServerVersion !== HERDR_SERVER_VERSION) fail(`Expected Herdr server ${HERDR_SERVER_VERSION}`);
const dockerVersion = spawnSync("docker", ["version", "--format", "{{.Server.Version}}"], { encoding: "utf8" });
if (dockerVersion.status !== 0) fail("Docker daemon unavailable");
const imageResult = spawnSync("docker", ["image", "inspect", IMAGE, "--format", "{{index .RepoDigests 0}}"], { encoding: "utf8" });
if (imageResult.status !== 0 || !imageResult.stdout.includes("sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3")) {
  fail(`Pinned worker image unavailable: ${IMAGE}`);
}

const root = mkdtempSync(path.join(os.tmpdir(), "capstan-m1-"));
const controllerDir = path.join(root, "controller-only");
mkdirSync(controllerDir, { mode: 0o700 });
const journalPath = path.join(controllerDir, "rpc-journal.jsonl");
const journalFd = openSync(journalPath, "a", 0o600);
const controllerFd = openSync(controllerDir, "r");
fsyncSync(controllerFd);
closeSync(controllerFd);
const rootFd = openSync(root, "r");
fsyncSync(rootFd);
closeSync(rootFd);
const containers = new Set();
let serial = 0;

function findBinary(name) {
  const result = spawnSync("which", [name], { encoding: "utf8" });
  if (result.status !== 0) fail(`${name} not found`);
  return result.stdout.trim();
}

function fail(message) {
  console.error(JSON.stringify({ result: "FAIL", reason: message }));
  process.exit(1);
}

function journal(entry) {
  const bytes = Buffer.from(`${JSON.stringify({ at: new Date().toISOString(), ...entry })}\n`);
  for (let offset = 0; offset < bytes.length;) {
    const written = writeSync(journalFd, bytes, offset, bytes.length - offset);
    if (written <= 0) throw new Error("Journal write made no progress");
    offset += written;
  }
  fsyncSync(journalFd);
}

function safeName(label) {
  return `capstan-m1-${process.pid}-${++serial}-${label.toLowerCase().replace(/[^a-z0-9-]/g, "-")}`;
}

function startWorker(role, workspace, home, hostSessionDir, suppressedReceiptId = null) {
  const name = safeName(role);
  containers.add(name);
  mkdirSync(workspace, { recursive: true, mode: 0o700 });
  mkdirSync(home, { recursive: true, mode: 0o700 });
  const sessionDir = `/home/worker/sessions/${role.toLowerCase()}`;
  mkdirSync(hostSessionDir, { recursive: true, mode: 0o700 });
  const profile = `m1-${role.toLowerCase()}`;
  const runtimeArgv = [
    "/usr/local/bin/omp", "--mode", "rpc", "--model", MODEL,
    "--profile", profile, "--cwd", "/workspace", "--session-dir", sessionDir,
    "--system-prompt", `You are the ${role} seat. Follow only the current user request.`,
    "--no-ui", "--no-extensions", "--no-skills", "--no-rules",
    ...(role === "Cancel" ? [] : ["--no-tools"]),
  ];
  const argv = [
    "run", "--interactive", "--name", name, "--label", "capstan.m1.probe=true",
    "--network", "bridge", "--user", `${process.getuid()}:${process.getgid()}`,
    "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
    "--mount", `type=bind,src=${home},dst=/home/worker`,
    "--mount", `type=bind,src=${workspace},dst=/workspace`,
    "--mount", `type=bind,src=${OMP},dst=/usr/local/bin/omp,readonly`,
    "--mount", `type=bind,src=${ADDON},dst=/usr/local/bin/pi_natives.linux-x64-baseline.node,readonly`,
    "--tmpfs", "/tmp:rw,nosuid,nodev",
    "--env", "HOME=/home/worker", "--env", "OPENAI_CODEX_OAUTH_TOKEN",
    IMAGE, ...runtimeArgv,
  ];
  const proc = spawn("docker", argv, {
    env: { ...process.env, OPENAI_CODEX_OAUTH_TOKEN: tokenResult.stdout.trim() },
    stdio: ["pipe", "pipe", "ignore"],
  });
  const frames = [];
  const waiters = [];
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  let pending = "";
  let closed = false;
  let exitCode = null;
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
    exitCode = code;
    for (const waiter of waiters.splice(0)) waiter.reject(wireError ?? new Error(`${role} worker exited before correlated frame (${code})`));
  });
  proc.stdout.on("data", (chunk) => {
    try {
      pending += decoder.decode(chunk, { stream: true });
      let end;
      while ((end = pending.indexOf("\n")) !== -1) {
        const line = pending.slice(0, end);
        pending = pending.slice(end + 1);
        if (Buffer.byteLength(line, "utf8") > 1_048_576) throw new Error(`${role} RPC frame exceeds 1 MiB`);
        const frame = JSON.parse(line);
        if (frame.id !== suppressedReceiptId) {
          journal({ event: "rpc_frame", role, name, type: frame.type, id: frame.id, success: frame.success, status: frame.status });
        }
        const index = waiters.findIndex((waiter) => waiter.predicate(frame));
        if (index >= 0) waiters.splice(index, 1)[0].resolve(frame);
        else {
          if (frames.length >= 64) throw new Error(`${role} queued too many RPC frames`);
          frames.push(frame);
        }
      }
      if (Buffer.byteLength(pending, "utf8") > 1_048_576) throw new Error(`${role} RPC frame exceeds 1 MiB`);
    } catch (error) { failWire(error); }
  });
  const waitFrame = (predicate, timeoutMs = 45_000) => {
    if (wireError) return Promise.reject(wireError);
    const index = frames.findIndex(predicate);
    if (index >= 0) return Promise.resolve(frames.splice(index, 1)[0]);
    if (closed) return Promise.reject(new Error(`${role} worker already exited (${exitCode})`));
    return new Promise((resolve, reject) => {
      const waiter = { predicate, resolve, reject };
      const timer = setTimeout(() => {
        const i = waiters.indexOf(waiter);
        if (i >= 0) waiters.splice(i, 1);
        reject(new Error(`${role} RPC frame timeout`));
      }, timeoutMs);
      waiter.resolve = (value) => { clearTimeout(timer); resolve(value); };
      waiter.reject = (error) => { clearTimeout(timer); reject(error); };
      waiters.push(waiter);
    });
  };
  const send = (command) => proc.stdin.write(`${JSON.stringify(command)}\n`);
  return {
    name,
    proc,
    waitFrame,
    send,
    async ready() {
      const frame = await waitFrame((item) => item.type === "ready");
      if (frame.protocolVersion !== 1) throw new Error("Unsupported OMP RPC protocol version");
      journal({ event: "ready", role, name, profile, cwd: "/workspace", sessionDir, argv: runtimeArgv });
      return frame;
    },
    async finish() {
      if (!closed) proc.stdin.end();
      if (!closed) await new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          proc.kill("SIGKILL");
          reject(new Error(`${role} worker did not close after RPC shutdown`));
        }, 30_000);
        proc.once("close", () => { clearTimeout(timer); resolve(); });
      });
      if (wireError) throw wireError;
      if (exitCode !== 0) throw new Error(`${role} OMP worker exit ${exitCode}`);
    },
  };
}

async function request(worker, command, id, persistReceipt = true) {
  journal({ event: "dispatch", role: command.role, commandId: id, rpcId: command.id, type: command.type });
  worker.send(command);
  const response = await worker.waitFrame((frame) => frame.type === "response" && frame.id === command.id);
  if (!response.success) throw new Error(`${command.type} RPC rejected (${response.error ?? "unknown error"})`);
  if (persistReceipt) journal({ event: "receipt", commandId: id, rpcId: command.id, type: command.type, success: true });
  return response;
}

function promptText(commandId, expected) {
  return `Command identity ${commandId}. Reply exactly ${expected}. Do not run tools.`;
}

async function completePrompt(role, expected, workspace, home, hostSessionDir, simulateReceiptLoss = false) {
  const commandId = `pm4-${role.toLowerCase()}-${expected.toLowerCase()}`;
  const worker = startWorker(role, workspace, home, hostSessionDir, simulateReceiptLoss ? commandId : null);
  await worker.ready();
  const prompt = promptText(commandId, expected);
  await request(worker, { id: commandId, role, type: "prompt", message: prompt }, commandId, !simulateReceiptLoss);
  const result = await worker.waitFrame((frame) => frame.type === "prompt_result" && frame.id === commandId);
  if (result.status !== "completed" || !result.sessionSettled) throw new Error(`${role} prompt did not settle successfully`);
  if (!simulateReceiptLoss) journal({ event: "prompt_result", commandId, status: result.status, sessionSettled: result.sessionSettled });
  if (!simulateReceiptLoss) {
    const readbackId = `${commandId}-readback`;
    const readback = await request(worker, { id: readbackId, role, type: "get_last_assistant_text" }, readbackId);
    if (readback.data?.text !== expected) throw new Error(`${role} reply did not match its seat token`);
  }
  await worker.finish();
  return { role, expected, commandId, home, workspace, sessionDir: `/home/worker/sessions/${role.toLowerCase()}`, receiptLost: simulateReceiptLoss };
}

function dockerText(args) {
  const result = spawnSync("docker", args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
  if (result.status !== 0) throw new Error(`docker ${args[0]} failed (${result.status})`);
  return result.stdout.trim();
}

async function main() {
  journal({ event: "probe_started", omp: VERSION, herdrClient: HERDR_CLIENT_VERSION, herdrServer: HERDR_SERVER_VERSION, image: IMAGE });
  const roles = ["PM", "Developer", "Verifier", "Supervisor"];
  const sessions = new Map();
  for (const role of roles) {
    const roleRoot = path.join(root, role.toLowerCase());
    sessions.set(role, await completePrompt(
      role,
      `M1_${role.toUpperCase()}_ACK`,
      path.join(roleRoot, "workspace"),
      path.join(roleRoot, "home"),
      path.join(roleRoot, "home", "sessions", role),
      role === "PM",
    ));
  }

  const pm = sessions.get("PM");
  const beforeRecovery = readFileSync(journalPath, "utf8").trim().split("\n").map((line) => JSON.parse(line));
  const pendingDispatches = beforeRecovery.filter((entry) => entry.event === "dispatch" && entry.commandId === pm.commandId).length;
  const prematureReceipts = beforeRecovery.filter((entry) => entry.commandId === pm.commandId && ["receipt", "prompt_result"].includes(entry.event)).length;
  if (pendingDispatches !== 1 || prematureReceipts !== 0) throw new Error("Receipt-loss injection did not leave one pending dispatch");
  const reconnect = startWorker("PM", pm.workspace, pm.home, path.join(pm.home, "sessions", "pm"));
  await reconnect.ready();
  const openId = "pm4-reconcile-open";
  const opened = await request(reconnect, { id: openId, role: "PM", type: "open_session", sessionDir: pm.sessionDir }, openId);
  if (!opened.data?.resumed) throw new Error("OMP did not resume the existing session");
  const entriesId = "pm4-reconcile-entries";
  const entriesResponse = await request(reconnect, { id: entriesId, role: "PM", type: "get_entries" }, entriesId);
  const entries = entriesResponse.data?.entries;
  if (!Array.isArray(entries)) throw new Error("OMP returned no session history");
  const matchingUsers = entries.filter((entry) => {
    const contents = entry.message?.content;
    return entry.type === "message" && entry.message?.role === "user" && Array.isArray(contents)
      && contents.length === 1 && contents[0].type === "text" && contents[0].text === promptText(pm.commandId, "M1_PM_ACK");
  });
  const occurrences = matchingUsers.length;
  const matchingReplies = occurrences === 1 ? entries.filter((entry) => {
    const contents = entry.message?.content;
    return entry.type === "message" && entry.parentId === matchingUsers[0].id
      && entry.message?.role === "assistant" && Array.isArray(contents) && contents.length === 1
      && contents[0].type === "text" && contents[0].text === "M1_PM_ACK";
  }) : [];
  const lastTextId = "pm4-reconcile-text";
  const textResponse = await request(reconnect, { id: lastTextId, role: "PM", type: "get_last_assistant_text" }, lastTextId);
  if (occurrences !== 1 || matchingReplies.length !== 1 || textResponse.data?.text !== "M1_PM_ACK") {
    throw new Error(`Reconnected history mismatch (user entries=${occurrences}, linked replies=${matchingReplies.length}, exact last reply=${textResponse.data?.text === "M1_PM_ACK"})`);
  }
  journal({ event: "receipt", role: "PM", commandId: pm.commandId, reconciled: true, matchingHistoryEntries: occurrences, resent: false });
  await reconnect.finish();

  const cancelWorkspace = path.join(root, "cancel", "workspace");
  const cancelHome = path.join(root, "cancel", "home");
  const cancelSession = path.join(cancelHome, "sessions", "cancel");
  const cancel = startWorker("Cancel", cancelWorkspace, cancelHome, cancelSession);
  await cancel.ready();
  const cancelId = "pm4-cancel-probe";
  await request(cancel, { id: cancelId, role: "Cancel", type: "prompt", message: "Use the bash tool to run this exact foreground command: printf started > /workspace/cancel.started; sleep 120. Do not answer until the command finishes." }, cancelId);
  const cancelStarted = path.join(cancelWorkspace, "cancel.started");
  const activeDeadline = Date.now() + 60_000;
  while (!existsSync(cancelStarted) && Date.now() < activeDeadline) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  if (!existsSync(cancelStarted)) throw new Error("Cancelable prompt never reached the blocking bash command");
  journal({ event: "prompt_active", commandId: cancelId, blockingToolStarted: true });
  const abortId = "pm4-abort-probe";
  await request(cancel, { id: abortId, role: "Cancel", type: "abort" }, abortId);
  const aborted = await cancel.waitFrame((frame) => frame.type === "prompt_result" && frame.id === cancelId);
  if (aborted.status !== "aborted") throw new Error("OMP RPC abort did not produce an aborted result");
  journal({ event: "interrupted", commandId: cancelId, status: aborted.status });
  await cancel.finish();

  const containWorkspace = path.join(root, "contain", "workspace");
  const containHome = path.join(root, "contain", "home");
  const contain = startWorker("Containment", containWorkspace, containHome, path.join(containHome, "sessions", "contain"));
  await contain.ready();
  const writeId = "pm4-worker-write";
  await request(contain, { id: writeId, role: "Containment", type: "bash", command: "(while :; do printf x >> /workspace/old-writer.log; sleep 0.1; done) >/dev/null 2>&1 &" }, writeId);
  const oldWrites = path.join(containWorkspace, "old-writer.log");
  const deadline = Date.now() + 5000;
  while (!existsSync(oldWrites) && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 50));
  if (!existsSync(oldWrites)) throw new Error("Worker child process did not start in its workspace");
  journal({ event: "containment_requested", role: "Containment", name: contain.name });
  dockerText(["kill", "--signal", "KILL", contain.name]);
  await new Promise((resolve) => contain.proc.once("close", resolve));
  const stopped = dockerText(["inspect", "--format", "{{.State.Status}}|{{.State.ExitCode}}|{{.State.Pid}}", contain.name]);
  if (stopped !== "exited|137|0") throw new Error(`Old worker containment not proven (${stopped})`);
  const sizeAtStop = statSync(oldWrites).size;
  dockerText(["rm", contain.name]);
  containers.delete(contain.name);

  const replacementWorkspace = containWorkspace;
  const replacementHome = path.join(root, "replacement", "home");
  const replacement = startWorker("Replacement", replacementWorkspace, replacementHome, path.join(replacementHome, "sessions", "replacement"));
  await replacement.ready();
  const replacementWriteId = "pm4-replacement-write";
  await request(replacement, { id: replacementWriteId, role: "Replacement", type: "bash", command: "printf replacement > /workspace/replacement.started" }, replacementWriteId);
  if (readFileSync(path.join(replacementWorkspace, "replacement.started"), "utf8") !== "replacement") throw new Error("Replacement writer did not write its assigned workspace");
  await new Promise((resolve) => setTimeout(resolve, 400));
  if (statSync(oldWrites).size !== sizeAtStop) throw new Error("Old writer changed the workspace after replacement started");
  journal({ event: "replacement_verified", priorWorkerStatus: stopped, replacementWrite: true, oldWriterSizeStable: true });
  await replacement.finish();
  const crashRecovery = await runControllerRestartProbe({ image: IMAGE, omp: OMP, addon: ADDON, model: MODEL, token: tokenResult.stdout.trim(), root });

  const records = readFileSync(journalPath, "utf8").trim().split("\n").map((line) => JSON.parse(line));
  const pmDispatches = records.filter((entry) => entry.event === "dispatch" && entry.commandId === pm.commandId).length;
  const recoveredReceipts = records.filter((entry) => entry.event === "receipt" && entry.commandId === pm.commandId && entry.reconciled === true).length;
  if (pmDispatches !== 1 || recoveredReceipts !== 1) throw new Error(`Expected one durable dispatch and recovered receipt, found ${pmDispatches} dispatches and ${recoveredReceipts} receipts`);
  return {
    result: "PASS_WITH_GAPS",
    versions: { omp: VERSION, herdrClient: HERDR_CLIENT_VERSION, herdrServer: HERDR_SERVER_VERSION, docker: dockerVersion.stdout.trim(), image: IMAGE },
    roles: roles.map((role) => ({
      profile: `m1-${role.toLowerCase()}`,
      cwd: "/workspace",
      argv: ["omp", "--mode", "rpc", "--model", MODEL, "--profile", `m1-${role.toLowerCase()}`, "--cwd", "/workspace", "--session-dir", `/home/worker/sessions/${role.toLowerCase()}`],
      exactReply: true,
    })),
    correlatedReceipts: records.filter((entry) => entry.event === "receipt").length,
    reconciliation: {
      commandId: pm.commandId,
      matchingHistoryEntries: occurrences,
      receiptGapInjected: true,
      pendingDispatchesBeforeRecovery: pendingDispatches,
      receiptsBeforeRecovery: prematureReceipts,
      resent: false,
      controllerJournalDispatches: pmDispatches,
      receiptsAfterRecovery: recoveredReceipts,
    },
    controllerCrashRecovery: crashRecovery,
    interruption: aborted.status,
    containment: { stopped, replacementWrite: true, oldWriterSizeStable: true },
    authority: "Docker-owned OMP stdio RPC; workers are not Herdr-managed and Herdr auto-restore has no worker target",
    limitations: [
      "The injected receipt-loss check runs with the controller alive; the separate controller SIGKILL/restart boundary is reported below.",
      "History identity and last-assistant-text readback are observations, not an OMP command-bound receipt or a selected-bridge deduplication guarantee.",
      "The single writer-kill/replacement smoke does not satisfy the ten-run cgroup, mount-removal, and quiescence qualification gate frozen in docs/m0-experiment.md.",
      "Herdr independent-caller risk is accepted in DEC-003; the selected Herdr-hosted bridge is still unproven.",
      "Workers receive the model OAuth token and unrestricted bridge egress; hostile-worker credential exfiltration and network policy are not tested.",
    ],
  };
}

let failed = false;
let summary;
let evidenceDir;
try {
  summary = await main();
} catch (error) {
  failed = true;
  const reason = error instanceof Error ? error.message : String(error);
  try { journal({ event: "probe_failed", reason }); } catch {}
  console.error(JSON.stringify({ result: "FAIL", reason }));
  process.exitCode = 1;
} finally {
  try { closeSync(journalFd); } catch {}
  const cleanupFailures = [];
  for (const name of containers) {
    const removed = spawnSync("docker", ["rm", "-f", name], { stdio: "ignore" });
    if (removed.status !== 0) cleanupFailures.push(name);
  }
  if (cleanupFailures.length) {
    failed = true;
    process.exitCode = 1;
    console.error(JSON.stringify({ result: "FAIL", reason: "Docker cleanup incomplete", containers: cleanupFailures }));
  }
  try {
    evidenceDir = preserveJournals();
    console.error(JSON.stringify({ evidenceDir }));
  } catch (error) {
    failed = true;
    process.exitCode = 1;
    console.error(JSON.stringify({ result: "FAIL", reason: `Could not preserve probe journals: ${error instanceof Error ? error.message : String(error)}` }));
  }
  if (failed) {
    console.error(JSON.stringify({ evidenceRoot: root, warning: "Probe workspaces and OMP sessions may contain sensitive data; remove after inspection." }));
  } else {
    rmSync(root, { recursive: true, force: true });
  }
}
if (!failed) console.log(JSON.stringify({ ...summary, evidenceDir }));

#!/usr/bin/env node
import { createHash } from "node:crypto";
import { createServer, createConnection } from "node:net";
import {
  chmodSync, closeSync, constants, copyFileSync, existsSync, fsyncSync, fstatSync, lstatSync, mkdirSync,
  ftruncateSync, mkdtempSync, openSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, unlinkSync, writeSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { createReceiptFrameBuffer } from "./m1-receipt-frame.mjs";
import { isReceiptType, isTerminalReceipt, recoverAbortIntents, replayReceiptState } from "./m1-receipt-journal.mjs";
process.umask(0o077);

const SELF = fileURLToPath(import.meta.url);
const EGRESS_HELPER = path.join(path.dirname(SELF), "m1-egress-helper.mjs");
const HERDR_VERSION = "0.9.0";
const HERDR_SHA256 = "4fa1a01158dd8043da92d31b270780b0dcc10603038d9b61cac4d81ab63fb71f";
const HERDR_URL = "https://github.com/herdrdev/herdr/releases/download/v0.9.0/herdr-linux-x86_64";
const OMP_VERSION = "18.3.1";
const NODE_VERSION = "v24.6.0";
const IMAGE = "ubuntu@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3";
const ROLES = ["PM", "Developer", "Verifier", "Supervisor"];
const ACK_COUNT = 10;
const PROGRESS_COUNT = 10;
const REPLACEMENT_COUNT = 10;
const MODEL = process.env.M1_OMP_MODEL ?? "openai-codex/gpt-6-sol";
const PROVIDER = process.env.M1_TOKEN_PROVIDER ?? "openai-codex";
const LIMITS = { ackMs: 120_000, progressMs: 900_000, replacementMs: 30_000 };
const root = mkdtempSync(path.join(os.tmpdir(), "capstan-m1-herdr-"));
const evidenceRoot = path.join(root, "evidence");
const controllerRoot = path.join(root, "controller-only");
const journalPath = path.join(evidenceRoot, "qualification.jsonl");
const journalFd = (() => { mkdirSync(evidenceRoot, { recursive: true, mode: 0o700 }); return openSync(journalPath, "a", 0o600); })();
let selectedEgressHelper;
const containers = new Set();
const containerIds = new Map();
const networks = new Set();
const egressPolicies = new Map();
const diagnosticPanes = new Map();
const receiptDaemons = new Set();
let serial = 0;
let failed = false;
let terminalError = null;
let summary;

function syncDir(dir) { const fd = openSync(dir, "r"); try { fsyncSync(fd); } finally { closeSync(fd); } }
function record(event, fields = {}) {
  const bytes = Buffer.from(`${JSON.stringify({ event, monotonicNs: process.hrtime.bigint().toString(), ...fields })}\n`);
  for (let off = 0; off < bytes.length;) { const n = writeSync(journalFd, bytes, off, bytes.length - off); if (n <= 0) throw new Error("Evidence journal write made no progress"); off += n; }
  fsyncSync(journalFd);
}
function sha256(bytes) { return createHash("sha256").update(bytes).digest("hex"); }
function run(bin, args, options = {}) {
  const r = spawnSync(bin, args, { encoding: "utf8", maxBuffer: 16 * 1024 * 1024, ...options });
  if (r.error || r.status !== 0) throw new Error(`${bin} ${args.join(" ")} failed (${r.status ?? r.error}): ${(r.stderr ?? "").trim()}`);
  return (r.stdout ?? "").trim();
}
function egress(action, args) {
  const output = run(process.execPath, [selectedEgressHelper, action, ...args]);
  let data;
  try { data = JSON.parse(output); } catch { throw new Error(`Egress helper ${action} returned malformed JSON`); }
  record("egress_policy", { action, result: data });
  return data;
}
function monotonicMs() { return Number(process.hrtime.bigint()) / 1e6; }
function retireSeatPolicy(name, containerId) {
  const state = JSON.parse(run("docker", ["inspect", "--format", "{{json .State}}", name]));
  if (state.Running && !state.Paused) {
    try {
      run("docker", ["pause", name]);
    } catch (pauseError) {
      try { run("docker", ["kill", "--signal", "KILL", name]); }
      catch (killError) { throw new AggregateError([pauseError, killError], `Unable to contain live worker ${name}`); }
    }
  }
  const contained = JSON.parse(run("docker", ["inspect", "--format", "{{json .State}}", name]));
  if (contained.Running && !contained.Paused) throw new Error(`Worker ${name} remains live before egress policy removal`);
  const result = egress("cleanup-container", ["--container", containerId]);
  if (result.removed !== true) throw new Error("Egress helper did not confirm policy cleanup");
  egressPolicies.delete(name);
  record("seat_policy_retired_before_removal", { container: name, containerId, paused: contained.Paused, stopped: !contained.Running, result });
  return result;
}
function safeName(tag) { return `capstan-m1hq-${process.pid}-${++serial}-${tag.toLowerCase().replace(/[^a-z0-9-]/g, "-")}`; }
function fsyncFile(file) { const fd = openSync(file, "r"); try { fsyncSync(fd); } finally { closeSync(fd); } }
function independentReconcileClientGet(socketPath, commandId) {
  const source = `const net=require("node:net"); const s=net.createConnection(process.argv[1]); let b=""; const t=setTimeout(()=>process.exit(2),10000); s.on("connect",()=>s.write(JSON.stringify({type:"get",commandId:process.argv[2]})+"\\n")); s.on("data",d=>{b+=d; for(;;){const n=b.indexOf("\\n"); if(n<0)break; const x=JSON.parse(b.slice(0,n)); b=b.slice(n+1); if(x.commandId===process.argv[2]&&x.type==="completed"){clearTimeout(t); process.stdout.write(JSON.stringify(x)); s.end(); return;}}}); s.on("error",e=>{console.error(e.message);process.exit(1)});`;
  const output = run(process.execPath, ["-e", source, socketPath, commandId]);
  const result = JSON.parse(output);
  if (result.type !== "completed" || result.commandId !== commandId) throw new Error("Fresh reconciliation client could not read durable completion");
  return result;
}
function executable(bin, label) { if (!existsSync(bin)) throw new Error(`${label} binary missing: ${bin}`); return realpathSync(bin); }

function ensureHerdr() {
  let file = process.env.M1_HERDR_BIN;
  if (file) file = executable(file, "Herdr");
  else {
    file = path.join(root, "herdr-linux-x86_64");
    run("curl", ["--fail", "--location", "--silent", "--show-error", HERDR_URL, "--output", file]);
    chmodSync(file, 0o700);
    record("artifact_downloaded", { name: "herdr", url: HERDR_URL });
  }
  const digest = sha256(readFileSync(file));
  if (digest !== HERDR_SHA256) throw new Error(`Herdr 0.9.0 SHA-256 mismatch: ${digest}`);
  const version = run(file, ["--version"], { env: { ...process.env, HERDR_ENV: "1" } });
  if (version !== `herdr ${HERDR_VERSION}`) throw new Error(`Expected Herdr ${HERDR_VERSION}, got ${version}`);
  record("artifact_verified", { name: "herdr", version: HERDR_VERSION, sha256: digest, path: file });
  return file;
}
function ensureRuntime() {
  if (process.version !== NODE_VERSION) throw new Error(`Qualification host Node must be ${NODE_VERSION}; got ${process.version}`);
  const omp = executable(process.env.M1_OMP_BIN ?? run("sh", ["-lc", "command -v omp"]), "OMP");
  const addon = executable(process.env.M1_OMP_NATIVE_ADDON ?? path.join(os.homedir(), ".omp", "natives", OMP_VERSION, "pi_natives.linux-x64-baseline.node"), "OMP native addon");
  const ompVersion = run(omp, ["--help"]);
  if (ompVersion.split("\n", 1)[0] !== `omp v${OMP_VERSION}`) throw new Error(`Expected OMP ${OMP_VERSION}`);
  const token = run(omp, ["token", PROVIDER, "--raw"]);
  if (!token) throw new Error(`No OMP token for ${PROVIDER}`);
  const dockerVersion = run("docker", ["version", "--format", "{{.Server.Version}}"]);
  const imageDigest = run("docker", ["image", "inspect", IMAGE, "--format", "{{json .RepoDigests}}"]);
  if (!imageDigest.includes(IMAGE.split("@")[1])) throw new Error(`Pinned image digest unavailable: ${IMAGE}`);
  const herdr = ensureHerdr();
  record("runtime_verified", { node: process.version, omp: OMP_VERSION, herdr: HERDR_VERSION, image: IMAGE, docker: dockerVersion, ompSha256: sha256(readFileSync(omp)), addonSha256: sha256(readFileSync(addon)), herdrSha256: HERDR_SHA256 });
  return { herdr, omp, addon, token, dockerVersion };
}

function scanTree(rootDir) {
  const rootStat = lstatSync(rootDir);
  if (!rootStat.isDirectory() || rootStat.isSymbolicLink()) throw new Error(`Invalid scan root ${rootDir}`);
  const rootReal = realpathSync(rootDir);
  const permittedDirectories = new Set([".home"]);
  const permittedFiles = new Set([".home/bridge.jsonl", "writer.log", "progress.jsonl", "replacement.started"]);
  const mountPoints = new Set(readFileSync("/proc/self/mountinfo", "utf8").split("\n").map((line) => line.split(" ")[4]?.replace(/\\040/g, " ").replace(/\\011/g, "\t").replace(/\\134/g, "\\")));
  for (const mountPoint of mountPoints) if (mountPoint && mountPoint.startsWith(`${rootReal}${path.sep}`)) throw new Error(`Nested mount point in workspace: ${mountPoint}`);
  const seenInodes = new Set();
  const entries = [];
  const rootFd = openSync(rootDir, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW | constants.O_CLOEXEC);
  const walk = (dirFd, rel) => {
    const beforeDir = fstatSync(dirFd);
    if (!beforeDir.isDirectory() || beforeDir.dev !== rootStat.dev) throw new Error(`Mount or directory replacement in workspace: ${rel}`);
    const dir = `/proc/self/fd/${dirFd}`;
    for (const name of readdirSync(dir).sort()) {
      if (name === "." || name === ".." || name.includes("/")) throw new Error(`Invalid workspace entry name: ${name}`);
      const full = `${dir}/${name}`;
      const itemRel = rel ? `${rel}/${name}` : name;
      const st = lstatSync(full);
      if (st.dev !== rootStat.dev) throw new Error(`Mount point in workspace: ${itemRel}`);
      if (st.isSymbolicLink()) throw new Error(`Symlink in workspace manifest: ${itemRel}`);
      if (st.isDirectory()) {
        if (!permittedDirectories.has(itemRel)) throw new Error(`Unauthorized workspace directory: ${itemRel}`);
        const childFd = openSync(full, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW | constants.O_CLOEXEC);
        try {
          const opened = fstatSync(childFd);
          if (opened.dev !== st.dev || opened.ino !== st.ino) throw new Error(`Directory changed during manifest scan: ${itemRel}`);
          entries.push({ path: itemRel, type: "dir", mode: st.mode & 0o777 });
          walk(childFd, itemRel);
          const after = lstatSync(full);
          if (!after.isDirectory() || after.dev !== st.dev || after.ino !== st.ino) throw new Error(`Directory changed during manifest scan: ${itemRel}`);
        } finally { closeSync(childFd); }
        continue;
      }
      if (!st.isFile()) throw new Error(`Special file in workspace manifest: ${itemRel}`);
      if (!permittedFiles.has(itemRel)) throw new Error(`Unauthorized workspace path: ${itemRel}`);
      if (st.nlink !== 1) throw new Error(`Hardlink in workspace manifest: ${itemRel}`);
      const mode = st.mode & 0o777;
      if (![0o600, 0o644, 0o755].includes(mode)) throw new Error(`Unauthorized file mode ${mode.toString(8)}: ${itemRel}`);
      const inode = `${st.dev}:${st.ino}`;
      if (seenInodes.has(inode)) throw new Error(`Duplicate inode: ${itemRel}`);
      seenInodes.add(inode);
      const fd = openSync(full, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_CLOEXEC);
      try {
        const opened = fstatSync(fd);
        if (!opened.isFile() || opened.dev !== st.dev || opened.ino !== st.ino || opened.nlink !== 1) throw new Error(`File changed during manifest scan: ${itemRel}`);
        const bytes = readFileSync(fd);
        const after = lstatSync(full);
        const finalStat = fstatSync(fd);
        if (!after.isFile() || after.dev !== st.dev || after.ino !== st.ino || after.size !== st.size || finalStat.size !== st.size) throw new Error(`File changed during manifest scan: ${itemRel}`);
        entries.push({ path: itemRel, type: "file", mode, size: st.size, sha256: sha256(bytes) });
      } finally { closeSync(fd); }
    }
    const afterDir = fstatSync(dirFd);
    if (afterDir.dev !== beforeDir.dev || afterDir.ino !== beforeDir.ino) throw new Error(`Directory changed during manifest scan: ${rel}`);
  };
  try {
    const openedRoot = fstatSync(rootFd);
    if (openedRoot.dev !== rootStat.dev || openedRoot.ino !== rootStat.ino) throw new Error("Workspace scan root replaced before traversal");
    walk(rootFd, "");
    const finalRoot = lstatSync(rootDir);
    if (finalRoot.dev !== rootStat.dev || finalRoot.ino !== rootStat.ino || realpathSync(rootDir) !== rootReal) throw new Error("Workspace scan root changed during manifest traversal");
  } finally { closeSync(rootFd); }
  return { entries, sha256: sha256(Buffer.from(JSON.stringify(entries))) };
}

function recoverControllerJournal(file, role) {
  const fd = openSync(file, constants.O_RDWR | constants.O_NOFOLLOW | constants.O_CLOEXEC);
  try {
    const st = fstatSync(fd);
    if (!st.isFile() || st.nlink !== 1) throw new Error("Controller receipt journal is not a private regular file");
    const bytes = readFileSync(fd);
    const completeEnd = bytes.lastIndexOf(0x0a) + 1;
    const entries = [];
    const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
    for (let start = 0; start < completeEnd;) {
      const end = bytes.indexOf(0x0a, start);
      let entry;
      try { entry = JSON.parse(decoder.decode(bytes.subarray(start, end))); }
      catch { throw new Error(`Malformed complete receipt journal record at byte ${start}`); }
      if (!entry || typeof entry !== "object" || Array.isArray(entry) || entry.role !== role
        || entry.sequence !== entries.length + 1 || typeof entry.commandId !== "string"
        || !isReceiptType(entry.type))
        throw new Error(`Invalid receipt journal record at byte ${start}`);
      entries.push(entry);
      start = end + 1;
    }
    if (completeEnd !== bytes.length) {
      ftruncateSync(fd, completeEnd);
      fsyncSync(fd);
      record("controller_receipt_tail_repaired", { file, truncatedBytes: bytes.length - completeEnd, survivingSequence: entries.length });
    }
    return entries;
  } finally { closeSync(fd); }
}

function verifyCompletionFrame(frame, spec) {
  if (frame?.type !== "completed" || frame.evidenceRef?.journal !== "/workspace/.home/bridge.jsonl"
    || !Number.isSafeInteger(frame.evidenceRef.sequence) || frame.evidenceRef.sequence < 1)
    throw new Error("Completion has no valid controller-owned receipt reference");
  const journal = recoverControllerJournal(spec.journal, spec.role);
  const entry = journal[frame.evidenceRef.sequence - 1];
  if (entry?.type !== "completed" || entry.sequence !== frame.evidenceRef.sequence
    || entry.commandId !== frame.commandId || entry.assignmentId !== frame.assignmentId
    || entry.attempt !== frame.attempt || entry.generation !== frame.generation || entry.reply !== frame.reply
    || JSON.stringify(entry.evidenceRef) !== JSON.stringify(frame.evidenceRef))
    throw new Error("Completion does not match fsynced controller journal");
  return frame;
}

function makeWorkspace(role, label, restoredWorkspace = null) {
  const base = path.join(root, label);
  const workspace = restoredWorkspace ?? path.join(base, "workspace");
  const bridge = path.join(base, "bridge");
  const journal = path.join(controllerRoot, `${label}.bridge.jsonl`);
  const receiptDir = path.join(controllerRoot, `${label}.receipt`);
  mkdirSync(controllerRoot, { recursive: true, mode: 0o700 });
  const journalFd = openSync(journal, "wx", 0o600);
  fsyncSync(journalFd);
  closeSync(journalFd);
  mkdirSync(receiptDir, { mode: 0o700 });
  syncDir(controllerRoot);
  mkdirSync(workspace, { recursive: true, mode: 0o700 });
  if (!restoredWorkspace) mkdirSync(path.join(workspace, ".home"), { mode: 0o700 });
  mkdirSync(bridge, { recursive: true, mode: 0o700 });
  if (workspace.startsWith(controllerRoot)) throw new Error("Controller state mount boundary failure");
  return { workspace, bridge, journal, receiptDir, role, label };
}

function cgroupPathFor(container) {
  const pid = Number(run("docker", ["inspect", "--format", "{{.State.Pid}}", container]));
  if (!Number.isSafeInteger(pid) || pid <= 0) throw new Error(`No live container init PID for ${container}`);
  const rows = readFileSync(`/proc/${pid}/cgroup`, "utf8").trim().split("\n");
  const row = rows.find((line) => line.startsWith("0::"));
  if (!row) throw new Error(`Unified cgroup unavailable for container ${container}`);
  const relative = row.slice(3).replace(/^\/+/, "");
  const candidates = [path.join("/sys/fs/cgroup", relative), `/sys/fs/cgroup/docker/${container}`];
  const cgroup = candidates.find((candidate) => existsSync(path.join(candidate, "cgroup.events")));
  if (!cgroup) throw new Error(`Cannot inspect cgroup for ${container}`);
  return { pid, cgroup };
}
function cgroupSnapshot(container, location, state) {
  if (state.Running || state.Pid !== 0) throw new Error(`Container still has a live init PID for ${container}`);
  if (!existsSync(path.join(location.cgroup, "cgroup.events"))) {
    record("cgroup_observed", { container, pid: location.pid, cgroup: location.cgroup, pids: 0, events: "removed", stoppedPid: state.Pid });
    return { pid: location.pid, cgroup: location.cgroup, pids: 0, events: "removed" };
  }
  const events = readFileSync(path.join(location.cgroup, "cgroup.events"), "utf8");
  const pids = Number(readFileSync(path.join(location.cgroup, "pids.current"), "utf8").trim());
  record("cgroup_observed", { container, pid: location.pid, cgroup: location.cgroup, pids, events });
  if (pids !== 0 || !/^populated 0$/m.test(events)) throw new Error(`Container cgroup still populated (${pids} PIDs)`);
  return { pid: location.pid, cgroup: location.cgroup, pids, events };
}

async function createSeat(runtime, spec, { ignoreStop = false } = {}) {
  const launchedAt = monotonicMs();
  const name = safeName(spec.label);
  const network = safeName(`${spec.label}-net`);
  run("docker", ["network", "create", "--internal", "--label", "capstan.m1.qualification=true", network]);
  networks.add(network);
  const providerHost = process.env.M1_PROVIDER_HOST;
  const providerPort = process.env.M1_PROVIDER_PORT ?? "443";
  if (!providerHost || !/^[a-z0-9.-]+$/i.test(providerHost) || !/^\d+$/.test(providerPort) || Number(providerPort) < 1 || Number(providerPort) > 65535) throw new Error("Set exact M1_PROVIDER_HOST and valid M1_PROVIDER_PORT for provider allowlisting");
  const socketPath = path.join(spec.bridge, "seat.sock");
  let bridgeSocket;
  const receiptPath = path.join(spec.receiptDir, "receipt.sock");
  const dispatched = new Map();
  let currentDispatch = null;
  const abortIntentPath = path.join(controllerRoot, `${spec.label}.abort-intent.jsonl`);
  const authorizedAborts = new Set();
  let receiptSequence = 0;
  let lastReceipt = null;
  let injectCompletionDrop = spec.label === "progress-01";
  let receiptFailed = false;
  let receiptServer;
  let expectedOMPHostPid = null;
  let deniedPeerCount = 0;
  let receiptFd = openSync(spec.journal, "a", 0o600);
  const receiptSockets = new Set();
  receiptServer = createServer({ allowHalfOpen: true }, (socket) => {
    const acceptedFd = socket._handle?.fd;
    if (!Number.isSafeInteger(acceptedFd) || acceptedFd < 0 || !expectedOMPHostPid) {
      socket.destroy();
      return;
    }
    const peer = spawnSync(runtime.peerAuth, [], { stdio: ["ignore", "pipe", "pipe", acceptedFd], encoding: "utf8", timeout: 5_000 });
    const observedPid = Number(peer.stdout?.trim());
    if (peer.status !== 0 || !Number.isSafeInteger(observedPid) || observedPid !== expectedOMPHostPid) {
      deniedPeerCount++;
      record("receipt_peer_denied", { container: name, observedPid: Number.isSafeInteger(observedPid) ? observedPid : null,
        expectedOMPHostPid, diagnostic: peer.stderr?.trim().slice(0, 200) });
      socket.destroy();
      return;
    }
    receiptSockets.add(socket);
    socket.once("close", () => receiptSockets.delete(socket));
    socket.setTimeout(10_000, () => socket.destroy(new Error("Receipt request timed out")));
    socket.on("error", () => {});
    let writing = false;
    let replied = false;
    const frameBuffer = createReceiptFrameBuffer(1_048_576);
    let pendingLine;
    let framingError;
    socket.on("data", (chunk) => {
      if (replied) { socket.destroy(); return; }
      if (framingError) return;
      try {
        const line = frameBuffer.push(chunk);
        if (line !== null) pendingLine = line;
      } catch (error) { framingError = error; }
    });
    socket.once("end", () => {
      if (replied) return;
      replied = true;
      if (framingError || pendingLine === undefined) {
        const error = framingError ?? new Error("receipt frame ended before newline delimiter");
        socket.end(`${JSON.stringify({ ok: false, error: String(error?.message ?? error) })}\n`);
        return;
      }
      try {
        if (receiptFailed) throw new Error("receipt journal is poisoned after a failed durable write");
        const entry = JSON.parse(pendingLine);
        if (!entry || typeof entry !== "object" || Array.isArray(entry) || entry.role !== spec.role
          || !isReceiptType(entry.type) || typeof entry.commandId !== "string")
          throw new Error("invalid receipt type/role/identity");
        if (entry.sequence === receiptSequence && lastReceipt && JSON.stringify(entry) === JSON.stringify(lastReceipt)) {
          writing = true;
          fsyncSync(receiptFd);
          socket.end(`${JSON.stringify({ ok: true, sequence: receiptSequence })}\n`);
          return;
        }
        if (entry.sequence !== receiptSequence + 1) throw new Error("invalid receipt sequence");
        const identity = { commandId: entry.commandId, assignmentId: entry.assignmentId, attempt: entry.attempt, generation: entry.generation };
        const dispatchedIdentity = dispatched.get(entry.commandId);
        if (!dispatchedIdentity || identity.commandId !== dispatchedIdentity.commandId
          || identity.assignmentId !== dispatchedIdentity.assignmentId || identity.attempt !== dispatchedIdentity.attempt
          || identity.generation !== dispatchedIdentity.generation || currentDispatch?.commandId !== entry.commandId)
          throw new Error("receipt identity was not the active controller dispatch");
        if (entry.type === "accepted") {
          if (dispatchedIdentity.accepted) throw new Error("duplicate accepted receipt");
          if (typeof entry.prompt !== "string" || entry.prompt !== currentDispatch.prompt) throw new Error("accepted receipt payload mismatch");
          dispatchedIdentity.accepted = true;
        } else if (!dispatchedIdentity.accepted) throw new Error("receipt precedes accepted dispatch");
        if (entry.type === "aborted" && !authorizedAborts.has(entry.commandId))
          throw new Error("abort lacks durable controller authorization");
        if (entry.type === "completed" && injectCompletionDrop) {
          injectCompletionDrop = false;
          record("completion_receipt_transport_dropped", { container: name, commandId: entry.commandId, sequence: entry.sequence });
          socket.destroy();
          return;
        }
        const bytes = Buffer.from(`${JSON.stringify(entry)}\n`);
        writing = true;
        for (let offset = 0; offset < bytes.length;) {
          const count = writeSync(receiptFd, bytes, offset, bytes.length - offset);
          if (count <= 0) throw new Error("receipt journal write made no progress");
          offset += count;
        }
        fsyncSync(receiptFd);
        receiptSequence = entry.sequence;
        lastReceipt = entry;
        if (isTerminalReceipt(entry.type)) currentDispatch = null;
        socket.end(`${JSON.stringify({ ok: true, sequence: receiptSequence })}\n`);
      } catch (error) {
        if (writing) receiptFailed = true;
        socket.end(`${JSON.stringify({ ok: false, error: String(error?.message ?? error) })}\n`);
      }
    });
  });
  try {
    await new Promise((resolve, reject) => {
      receiptServer.once("error", reject);
      receiptServer.listen(receiptPath, () => { receiptServer.off("error", reject); resolve(); });
    });
    chmodSync(receiptPath, 0o600);
  } catch (error) {
    if (receiptServer.listening) await new Promise((resolve) => receiptServer.close(resolve));
    closeSync(receiptFd);
    try { unlinkSync(receiptPath); } catch (cleanupError) { if (cleanupError?.code !== "ENOENT") throw new AggregateError([error, cleanupError], "Receipt daemon startup and cleanup failed"); }
    throw error;
  }
  receiptServer.on("error", () => { receiptFailed = true; });
  receiptDaemons.add({ server: receiptServer, fd: receiptFd, path: receiptPath, journal: spec.journal, sockets: receiptSockets });
  const frames = [];
  const waiters = [];
  let socketError;
  let lineBuffer = "";
  let controllerConnectedAt;
  const deliver = (frame) => {
    const index = waiters.findIndex((w) => w.predicate(frame));
    if (index !== -1) waiters.splice(index, 1)[0].resolve(frame);
    else { if (frames.length >= 10_000) throw new Error("Bridge socket frame queue overflow"); frames.push(frame); }
  };
  function acceptSocket(socket) {
    bridgeSocket = socket;
    socketError = undefined;
    lineBuffer = "";
    const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
    socket.on("data", (chunk) => {
      try {
        lineBuffer += decoder.decode(chunk, { stream: true });
        if (Buffer.byteLength(lineBuffer, "utf8") > 1_048_576 && !lineBuffer.includes("\n")) throw new Error("Bridge frame exceeds 1 MiB");
        let end;
        while ((end = lineBuffer.indexOf("\n")) >= 0) {
          const line = lineBuffer.slice(0, end); lineBuffer = lineBuffer.slice(end + 1);
          if (Buffer.byteLength(line, "utf8") > 1_048_576) throw new Error("Bridge frame exceeds 1 MiB");
          const parsed = JSON.parse(line);
          if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("Bridge response must be a JSON object");
          deliver({ ...parsed, _arrivalMonotonicMs: monotonicMs() });
        }
      } catch (error) { socketError = error; socket.destroy(error); }
    });
    socket.on("error", (error) => { socketError ??= error; });
    socket.on("close", () => {
      try { lineBuffer += decoder.decode(); if (lineBuffer) throw new Error("Unterminated bridge response frame"); }
      catch (error) { socketError ??= error; }
      if (bridgeSocket === socket) bridgeSocket = undefined;
      if (socketError) for (const waiter of waiters.splice(0)) waiter.reject(socketError);
    });
  }
  async function connectController(timeoutMs = 30_000) {
    const deadline = monotonicMs() + timeoutMs;
    let lastError;
    while (monotonicMs() < deadline) {
      try {
        const st = lstatSync(socketPath);
        if (!st.isSocket()) throw new Error("Bridge endpoint is not a Unix socket");
        await new Promise((resolve, reject) => {
          const socket = createConnection(socketPath);
          socket.once("connect", () => {
            const fd = socket._handle?.fd;
            const peer = Number.isSafeInteger(fd) && fd >= 0
              ? spawnSync(runtime.peerAuth, [], { stdio: ["ignore", "pipe", "pipe", fd], encoding: "utf8", timeout: 5_000 })
              : null;
            if (!expectedOMPHostPid || peer?.status !== 0 || Number(peer.stdout?.trim()) !== expectedOMPHostPid) {
              socket.destroy();
              reject(new Error("Bridge server peer is not the selected Herdr-hosted OMP process"));
              return;
            }
            acceptSocket(socket);
            resolve();
          });
          socket.once("error", reject);
        });
        controllerConnectedAt = monotonicMs();
        record("controller_reconnected", { container: name, socketPath });
        return;
      } catch (error) { lastError = error; await new Promise((resolve) => setTimeout(resolve, 100)); }
    }
    throw new Error(`Selected OMP bridge did not expose a connectable Unix socket: ${lastError?.message ?? "no endpoint"}`);
  }
  async function ensureBridgeConnected() {
    if (!bridgeSocket || bridgeSocket.destroyed) await connectController();
  }
  const bridgeExtension = path.resolve(path.dirname(SELF), "m1-herdr-bridge.mjs");
  executable(bridgeExtension, "Selected Herdr bridge extension");
  const bridgeMount = `type=bind,src=${spec.bridge},dst=/bridge`;
  const wsMount = `type=bind,src=${spec.workspace},dst=/workspace`;
  const args = [
    "run", "--detach", "--name", name, "--label", "capstan.m1.qualification=true", "--network", network,
    "--user", `${process.getuid()}:${process.getgid()}`,
    "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "256",
    "--mount", `${wsMount},bind-propagation=rprivate`, "--mount", `${bridgeMount},bind-propagation=rprivate`,
    "--mount", `type=bind,src=${spec.journal},dst=/workspace/.home/bridge.jsonl,readonly,bind-propagation=rprivate`,
    "--mount", `type=bind,src=${spec.receiptDir},dst=/receipt,readonly,bind-propagation=rprivate`,
    "--tmpfs", `/home/worker:rw,nosuid,nodev,size=256m,uid=${process.getuid()},gid=${process.getgid()},mode=0700`,
    "--mount", `type=bind,src=${runtime.herdr},dst=/usr/local/bin/herdr,readonly`,
    "--mount", `type=bind,src=${runtime.omp},dst=/usr/local/bin/omp,readonly`,
    "--mount", `type=bind,src=${runtime.addon},dst=/usr/local/bin/pi_natives.linux-x64-baseline.node,readonly`,
    "--mount", `type=bind,src=${runtime.node},dst=/usr/local/bin/node,readonly`,
    "--mount", `type=bind,src=${bridgeExtension},dst=/usr/local/bin/m1-herdr-bridge.mjs,readonly`,
    "--mount", `type=bind,src=${runtime.peerAuth},dst=/usr/local/bin/m1-receipt-peer,readonly`,
    "--tmpfs", `/tmp:rw,nosuid,nodev,noexec,size=256m,uid=${process.getuid()},gid=${process.getgid()},mode=0700`,
    "--tmpfs", `/run:rw,nosuid,nodev,noexec,size=64m,uid=${process.getuid()},gid=${process.getgid()},mode=0700`,
    "--env", "HOME=/home/worker", "--env", "PATH=/usr/local/bin:/usr/bin:/bin",
    "--env", "LC_ALL=C", "--env", "LANG=C", "--env", "TZ=UTC", "--env", "TMPDIR=/tmp", "--env", "HERDR_ENV=1",
    "--env", "CAPSTAN_BRIDGE_SOCKET=/bridge/seat.sock", "--env", "CAPSTAN_BRIDGE_JOURNAL=/workspace/.home/bridge.jsonl",
    "--env", "CAPSTAN_BRIDGE_RECEIPT_SOCKET=/receipt/receipt.sock",
    "--env", "CAPSTAN_BRIDGE_PEER_HELPER=/usr/local/bin/m1-receipt-peer",
    "--env", "CAPSTAN_BRIDGE_CONTROLLER_PEER_PID=0",
    "--env", `CAPSTAN_BRIDGE_ROLE=${spec.role}`, "--env", "OPENAI_CODEX_OAUTH_TOKEN",
    IMAGE, "sh", "-c", ignoreStop ? "umask 077; trap '' TERM; while :; do sleep 60 & wait; done" : "umask 077; exec sleep infinity",
  ];
  const containerId = run("docker", args, { env: { ...process.env, OPENAI_CODEX_OAUTH_TOKEN: runtime.token } });
  containers.add(name);
  containerIds.set(name, containerId);
  const containerIp = run("docker", ["inspect", "--format", "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}", name]);
  const prepared = egress("prepare", ["--container", containerId, "--container-ip", containerIp, "--provider-host", providerHost, "--provider-port", providerPort, "--proxy-port", "0"]);
  if (typeof prepared.proxy !== "string" || !/^http:\/\/[^/]+:\d+$/.test(prepared.proxy) || typeof prepared.policyId !== "string") throw new Error("Egress helper returned invalid proxy/policy identity");
  egressPolicies.set(name, { policyId: prepared.policyId, network, providerHost, providerPort, containerId });
  const verified = egress("verify", ["--container", containerId, "--policy-id", prepared.policyId]);
  if (verified.firewall?.forwarding !== "DOCKER-USER accept-proxy-then-reject"
    || verified.firewall?.hostInput !== "INPUT accept-proxy-then-reject" || verified.providerConnections !== 0)
    throw new Error("Egress policy verification failed before OMP launch");
  const denied = egress("probe-deny", ["--container", containerId, "--policy-id", prepared.policyId]);
  if (denied.denied !== true || denied.hostDenied !== true)
    throw new Error("Out-of-policy direct egress or host INPUT probe was not denied");
  const proxyArgs = ["--env", `HTTP_PROXY=${prepared.proxy}`, "--env", `HTTPS_PROXY=${prepared.proxy}`, "--env", `http_proxy=${prepared.proxy}`, "--env", `https_proxy=${prepared.proxy}`, "--env", "NODE_USE_ENV_PROXY=1"];
  const exec = (argv, opts = {}) => run("docker", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, "--env", "HOME=/home/worker", ...proxyArgs, name, "/bin/sh", "-c", "umask 077; exec \"$@\"", "m1-qualification", ...argv], opts);
  try {
    const tamper = spawnSync("docker", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, name,
      "/bin/sh", "-c", "printf x >> /workspace/.home/bridge.jsonl"], { encoding: "utf8", timeout: 5_000 });
    if (tamper.status === 0 || !/Read-only file system/i.test(tamper.stderr)
      || statSync(spec.journal).size !== 0) throw new Error("Worker could alter the controller-owned receipt journal");
    record("worker_receipt_mutation_denied", { container: name, journal: spec.journal, exitCode: tamper.status });
    if (exec(["/usr/local/bin/node", "--version"]) !== NODE_VERSION) throw new Error("Container Node version mismatch");
    const replaceSocket = spawnSync("docker", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, name,
      "/bin/rm", "/receipt/receipt.sock"], { encoding: "utf8", timeout: 5_000 });
    if (replaceSocket.status === 0 || !/Read-only file system/i.test(replaceSocket.stderr)
      || !lstatSync(receiptPath).isSocket()) throw new Error("Worker could replace the controller-owned receipt socket");
    record("worker_receipt_socket_mutation_denied", { container: name, receiptPath, exitCode: replaceSocket.status });
    if (exec(["/usr/local/bin/herdr", "--version"]) !== `herdr ${HERDR_VERSION}`) throw new Error("Container Herdr version mismatch");
    if (!exec(["/usr/local/bin/omp", "--help"]).startsWith(`omp v${OMP_VERSION}\n`)) throw new Error("Container OMP version mismatch");
    exec(["/bin/mkdir", "-p", "/home/worker/.omp/agent/extensions"]);
    run("docker", ["exec", "--detach", "--user", `${process.getuid()}:${process.getgid()}`, "--env", "HOME=/home/worker",
      ...proxyArgs, name, "/usr/local/bin/herdr", "server"]);
    const serverDeadline = monotonicMs() + 15_000;
    while (true) {
      const status = spawnSync("docker", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, "--env", "HOME=/home/worker",
        name, "/usr/local/bin/herdr", "status", "server"], { encoding: "utf8" });
      if (status.status === 0 && status.stdout.includes("status: running")) break;
      if (monotonicMs() >= serverDeadline) throw new Error(`Isolated Herdr server did not start: ${status.stderr}`);
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    exec(["/usr/local/bin/herdr", "integration", "install", "omp"]);
    exec(["/usr/local/bin/node", "-e",
      "const fs=require('node:fs'); const path=require('node:path'); const file=path.join('/home/worker/.omp/profiles',process.argv[1],'agent/config.yml'); fs.mkdirSync(path.dirname(file),{recursive:true,mode:0o700}); fs.writeFileSync(file,'setupVersion: 2\\n',{mode:0o600});",
      `m1-${spec.role.toLowerCase()}`]);
    const created = JSON.parse(exec(["/usr/local/bin/herdr", "workspace", "create", "--cwd", "/workspace", "--no-focus"]));
    const pane = created.result?.root_pane?.pane_id;
    if (!/^w\d+:p\d+$/.test(pane)) throw new Error("Herdr did not create one private OMP pane");
    diagnosticPanes.set(name, pane);
    const ext = "/usr/local/bin/m1-herdr-bridge.mjs";
    const start = ["/usr/local/bin/herdr", "agent", "start", `m1_${spec.label}`, "--kind", "omp", "--pane", pane, "--", "--model", MODEL, "--profile", `m1-${spec.role.toLowerCase()}`, "--cwd", "/workspace", "--extension", ext];
    exec(start);
    const pidDeadline = monotonicMs() + 30_000;
    let processRows;
    let ompRows;
    do {
      processRows = run("docker", ["top", name, "-eo", "pid,args"]).split("\n");
      ompRows = processRows.slice(1).filter((line) => line.includes(ext) && /\somp --model /.test(line)
        && line.includes(`--profile m1-${spec.role.toLowerCase()}`));
      if (ompRows.length === 1) break;
      await new Promise((resolve) => setTimeout(resolve, 100));
    } while (monotonicMs() < pidDeadline);
    if (ompRows.length !== 1) throw new Error(`Expected one Herdr-hosted OMP process for receipt authentication: ${JSON.stringify(processRows)}`);
    expectedOMPHostPid = Number(ompRows[0].trim().split(/\s+/, 1)[0]);
    if (!Number.isSafeInteger(expectedOMPHostPid) || expectedOMPHostPid <= 0) throw new Error("Invalid OMP host PID for receipt authentication");
    record("receipt_peer_bound", { container: name, hostPid: expectedOMPHostPid, extension: ext });
    await connectController();
    const probePeer = spawn("docker", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, name,
      "/usr/local/bin/node", "-e",
      "const s=require('node:net').createConnection('/receipt/receipt.sock'); s.on('error',()=>{}); s.on('close',()=>process.exit(0)); setTimeout(()=>process.exit(2),3000);"],
    { stdio: "ignore" });
    const probeExit = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { probePeer.kill(); reject(new Error("Worker peer-denial probe timed out")); }, 5_000);
      probePeer.once("error", (error) => { clearTimeout(timer); reject(error); });
      probePeer.once("exit", (code) => { clearTimeout(timer); resolve(code); });
    });
    if (probeExit !== 0 || deniedPeerCount !== 1 || statSync(spec.journal).size !== 0)
      throw new Error("A same-UID worker process was not denied receipt authority by Unix peer PID");
    record("worker_receipt_peer_denied", { container: name, expectedOMPHostPid, denials: deniedPeerCount });
    const untrustedBridgePeer = spawn("docker", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, name,
      "/usr/local/bin/node", "-e",
      "const s=require('node:net').createConnection('/bridge/seat.sock'); s.on('connect',()=>s.write(JSON.stringify({type:'get',commandId:'untrusted-worker'})+'\\n')); s.on('data',()=>process.exit(2)); s.on('error',(error)=>process.exit(error.code==='ECONNRESET'?0:3)); s.on('close',()=>process.exit(0)); setTimeout(()=>process.exit(4),3000);"],
    { stdio: ["ignore", "ignore", "pipe"] });
    let untrustedDiagnostic = "";
    untrustedBridgePeer.stderr.on("data", (bytes) => { untrustedDiagnostic += bytes.toString("utf8"); });
    const untrustedExit = await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { untrustedBridgePeer.kill(); reject(new Error("Worker bridge-peer denial probe timed out")); }, 5_000);
      untrustedBridgePeer.once("error", (error) => { clearTimeout(timer); reject(error); });
      untrustedBridgePeer.once("exit", (code) => { clearTimeout(timer); resolve(code); });
    });
    if (untrustedExit !== 0 || statSync(spec.journal).size !== 0)
      throw new Error(`A same-UID worker process could query or stall the controller bridge: exit ${untrustedExit}, diagnostic ${untrustedDiagnostic.slice(0, 500)}`);
    record("worker_bridge_peer_denied", { container: name, expectedControllerPeerPid: 0 });
  } catch (error) {
    const pane = diagnosticPanes.get(name);
    if (pane) {
      const diagnostic = spawnSync("docker", ["exec", name, "/usr/local/bin/herdr", "pane", "read", pane, "--source", "recent-unwrapped", "--lines", "120"],
        { encoding: "utf8", timeout: 5_000, maxBuffer: 1_000_000 });
      const file = path.join(evidenceRoot, `${name}.startup-pane.txt`);
      const bytes = Buffer.from(`${diagnostic.stdout ?? ""}\n${diagnostic.stderr ?? ""}`);
      const fd = openSync(file, "w", 0o600);
      try { writeSync(fd, bytes); fsyncSync(fd); } finally { closeSync(fd); }
      record("startup_diagnostic_preserved", { name, sha256: sha256(bytes), path: file });
    }
    for (const [label, args] of [
      ["startup-docker", ["logs", name]],
      ["startup-access", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, name, "/usr/local/bin/node", "-e",
        "const fs=require('node:fs'); for(const p of ['/workspace/.home/bridge.jsonl','/bridge','/bridge/receipt.sock','/bridge/seat.sock']) {try {const s=fs.lstatSync(p);console.log(p,s.mode.toString(8),s.uid,s.size); if(s.isFile()) console.log('read',fs.readFileSync(p).length);}catch(e){console.log(p,e.code,e.message)}}"]],
    ]) {
      const diagnostic = spawnSync("docker", args, { encoding: "utf8", timeout: 5_000, maxBuffer: 1_000_000 });
      const bytes = Buffer.from(`${diagnostic.stdout ?? ""}\n${diagnostic.stderr ?? ""}`);
      const file = path.join(evidenceRoot, `${name}.${label}.txt`);
      const fd = openSync(file, "w", 0o600);
      try { writeSync(fd, bytes); fsyncSync(fd); } finally { closeSync(fd); }
      record("startup_diagnostic_preserved", { name, label, sha256: sha256(bytes), path: file });
    }
    try {
      retireSeatPolicy(name, containerId);
      run("docker", ["rm", "-f", name]); containers.delete(name); containerIds.delete(name);
      run("docker", ["network", "rm", network]); networks.delete(network);
    } catch (cleanupError) { throw new AggregateError([error, cleanupError], "Seat startup and cleanup failed"); }
    throw error;
  }

  function waitFrame(predicate, timeoutMs = 45_000) {
    if (socketError) return Promise.reject(socketError);
    const index = frames.findIndex(predicate);
    if (index >= 0) return Promise.resolve(frames.splice(index, 1)[0]);
    return new Promise((resolve, reject) => {
      const waiter = { predicate, resolve: null, reject: null };
      const timer = setTimeout(() => { const i = waiters.indexOf(waiter); if (i >= 0) waiters.splice(i, 1); reject(new Error(`Bridge socket frame timeout (${timeoutMs}ms)`)); }, timeoutMs);
      waiter.resolve = (v) => { clearTimeout(timer); resolve(v); };
      waiter.reject = (e) => { clearTimeout(timer); reject(e); };
      waiters.push(waiter);
    });
  }
  function send(frame) {
    const line = `${JSON.stringify(frame)}\n`;
    if (Buffer.byteLength(line) > 1_048_576) throw new Error("Controller bridge request exceeds 1 MiB");
    if (frame.type === "dispatch") {
      const identity = { commandId: frame.commandId, assignmentId: frame.assignmentId, attempt: frame.attempt, generation: frame.generation };
      const prior = dispatched.get(frame.commandId);
      if (prior && (prior.assignmentId !== identity.assignmentId || prior.attempt !== identity.attempt || prior.generation !== identity.generation)) throw new Error("Controller attempted a conflicting dispatch identity");
      if (!prior) {
        dispatched.set(frame.commandId, { ...identity, prompt: frame.prompt, accepted: false });
        currentDispatch = { ...identity, prompt: frame.prompt };
      }
    }
    bridgeSocket.write(line);
  }
  function verifyProvider() {
    const policy = egressPolicies.get(name);
    if (!policy) throw new Error("Missing egress policy identity");
    const observed = egress("verify", ["--container", policy.containerId, "--policy-id", policy.policyId]);
    if (observed.firewall?.forwarding !== "DOCKER-USER accept-proxy-then-reject"
      || observed.firewall?.hostInput !== "INPUT accept-proxy-then-reject" || observed.providerConnections < 1)
      throw new Error("Selected OMP did not produce an observed allowlisted provider connection through the proxy");
    record("provider_egress_proven", { container: policy.containerId, host: policy.providerHost, port: policy.providerPort, observed });
    return observed;
  }
  return {
    async restartReceiptDaemon() {
      const idleClient = createConnection(receiptPath);
      idleClient.on("error", () => {});
      await new Promise((resolve, reject) => { idleClient.once("connect", resolve); idleClient.once("error", reject); });
      const idleClosed = new Promise((resolve) => idleClient.once("close", resolve));
      for (const socket of receiptSockets) socket.destroy();
      let idleTimeout;
      try {
        await Promise.race([idleClosed, new Promise((_, reject) => {
          idleTimeout = setTimeout(() => reject(new Error("Idle receipt connection survived daemon restart")), 5_000);
        })]);
      } finally { clearTimeout(idleTimeout); }
      record("idle_receipt_connection_contained", { container: name });
      await new Promise((resolve) => receiptServer.close(resolve));
      if (existsSync(receiptPath)) unlinkSync(receiptPath);
      closeSync(receiptFd);
      const recovered = replayReceiptState(recoverControllerJournal(spec.journal, spec.role));
      dispatched.clear();
      for (const [id, command] of recovered.commands) dispatched.set(id, command);
      currentDispatch = recovered.active;
      receiptSequence = recovered.sequence;
      lastReceipt = recovered.lastReceipt;
      receiptFailed = false;
      authorizedAborts.clear();
      if (existsSync(abortIntentPath)) {
        const fd = openSync(abortIntentPath, constants.O_RDWR | constants.O_NOFOLLOW | constants.O_CLOEXEC);
        try {
          const stat = fstatSync(fd);
          if (!stat.isFile() || stat.nlink !== 1) throw new Error("Controller abort intent is not a private regular file");
          const bytes = readFileSync(fd);
          const { authorized, completeEnd } = recoverAbortIntents(bytes, dispatched);
          if (completeEnd !== bytes.length) {
            ftruncateSync(fd, completeEnd);
            fsyncSync(fd);
            record("controller_abort_intent_tail_repaired", { file: abortIntentPath, truncatedBytes: bytes.length - completeEnd });
          }
          for (const id of authorized) authorizedAborts.add(id);
        } finally { closeSync(fd); }
      }
      receiptFd = openSync(spec.journal, "a", 0o600);
      const receipt = [...receiptDaemons].find((item) => item.path === receiptPath);
      if (!receipt) throw new Error("Controller receipt daemon not registered for restart");
      receipt.fd = receiptFd;
      await new Promise((resolve, reject) => {
        receiptServer.once("error", reject);
        receiptServer.listen(receiptPath, () => { receiptServer.off("error", reject); resolve(); });
      });
      chmodSync(receiptPath, 0o600);
      record("controller_receipt_daemon_restarted", { container: name, journal: spec.journal,
        sequence: receiptSequence, activeCommandId: currentDispatch?.commandId ?? null, recoveredCommands: dispatched.size });
    },
    controllerConnectedAt,
    name, spec, waitFrame, send, launchedAt,
    async reconnectController(timeoutMs = 30_000) {
      const prior = bridgeSocket;
      bridgeSocket = undefined;
      if (prior && !prior.destroyed) await new Promise((resolve) => { prior.once("close", resolve); prior.destroy(); });
      await connectController(timeoutMs);
    },
    async reconcileDuplicate(dispatch, expectedEvidenceRef) {
      await ensureBridgeConnected();
      send(dispatch);
      const duplicateResult = await waitFrame((x) => x.commandId === dispatch.commandId && ["completed", "unknown"].includes(x.type));
      if (duplicateResult.type === "unknown") throw new Error(`Duplicate dispatch ${dispatch.commandId} is unknown`);
      verifyCompletionFrame(duplicateResult, spec);
      if (JSON.stringify(duplicateResult.evidenceRef) !== JSON.stringify(expectedEvidenceRef)) throw new Error("Duplicate dispatch changed durable evidence identity");
      await this.reconnectController();
      const replay = await this.query(dispatch.commandId, 10_000, "completed");
      if (JSON.stringify(replay.evidenceRef) !== JSON.stringify(expectedEvidenceRef)) throw new Error("get replay changed durable result identity");
      return { result: duplicateResult, replay };
    },
    verifyProvider,
    async command({ commandId, assignmentId, attempt = 1, generation = 1, prompt, deadlineMs = 120_000 }) {
      const started = launchedAt;
      const dispatch = { type: "dispatch", commandId, assignmentId, attempt, generation, prompt };
      if (dispatched.has(commandId) || currentDispatch) throw new Error("Controller already owns an active or previously dispatched command identity");
      await ensureBridgeConnected();
      send(dispatch);
      const ack = await waitFrame((x) => x.type === "ack" && x.commandId === commandId, deadlineMs);
      if (ack.durable !== true || !["acknowledged", "working", "completed"].includes(ack.state)) throw new Error(`Non-durable/invalid acknowledgement for ${commandId}`);
      const ackMs = monotonicMs() - started;
      record("ack_sample", { commandId, role: spec.role, assignmentId, attempt, generation, milliseconds: ackMs, durable: ack.durable, state: ack.state });
      if (ackMs > LIMITS.ackMs) throw new Error(`Acknowledgement exceeded 120s (${ackMs}ms)`);
      return { ack, ackMs };
    },
    async lifecycle(commandId, types, timeoutMs) {
      const frame = await waitFrame((x) => x.commandId === commandId && (x.type === "unknown" || types.includes(x.type)), timeoutMs);
      if (frame.type === "unknown") throw new Error(`Command ${commandId} became unknown before ${types.join("/")}`);
      return frame;
    },
    async completed(commandId, timeoutMs = 900_000) {
      const frame = await this.lifecycle(commandId, ["completed"], timeoutMs);
      if (typeof frame.reply !== "string" || !frame.reply.length || !frame.evidenceRef || typeof frame.evidenceRef !== "object") throw new Error(`Completion missing reply/evidenceRef for ${commandId}`);
      verifyCompletionFrame(frame, spec);
      return frame;
    },
    async query(commandId, timeoutMs = 10_000, expectedType = null) {
      await ensureBridgeConnected();
      send({ type: "get", commandId });
      const frame = await waitFrame((x) => x.commandId === commandId
        && ((x.type === "ack" && x.state === "unknown") || (expectedType ? x.type === expectedType : ["ack", "completed"].includes(x.type))), timeoutMs);
      if (frame.type === "ack" && frame.state === "unknown") throw new Error(`Command ${commandId} is unknown`);
      if (frame.type === "completed") verifyCompletionFrame(frame, spec);
      return frame;
    },
    async abort(commandId, timeoutMs = 30_000) {
      await ensureBridgeConnected();
      if (currentDispatch?.commandId !== commandId || authorizedAborts.has(commandId))
        throw new Error("Abort requires one active controller dispatch");
      const bytes = Buffer.from(`${JSON.stringify({ commandId, ...dispatched.get(commandId) })}\n`);
      const fd = openSync(abortIntentPath, "a", 0o600);
      try {
        const originalSize = fstatSync(fd).size;
        try {
          for (let offset = 0; offset < bytes.length;) {
            const written = writeSync(fd, bytes, offset, bytes.length - offset);
            if (written <= 0) throw new Error("Abort-intent write made no progress");
            offset += written;
          }
          fsyncSync(fd);
        } catch (error) {
          try { ftruncateSync(fd, originalSize); fsyncSync(fd); }
          catch (rollbackError) { throw new AggregateError([error, rollbackError], "Abort-intent write failed and partial-tail rollback failed"); }
          throw error;
        }
      } finally { closeSync(fd); }
      syncDir(controllerRoot);
      authorizedAborts.add(commandId);
      record("controller_abort_authorized", { container: name, commandId, intentPath: abortIntentPath });
      send({ type: "abort", commandId });
      return waitFrame((x) => x.commandId === commandId && x.type === "ack" && x.state === "unknown" && x.durable === true, timeoutMs);
    },
    async probeUnauthorizedAbort(commandId) {
      if (currentDispatch?.commandId !== commandId || existsSync(abortIntentPath))
        throw new Error("Unauthorized abort probe requires active work without controller intent");
      const child = spawn("docker", ["exec", "--user", `${process.getuid()}:${process.getgid()}`, name,
        "/usr/local/bin/node", "-e",
        "const s=require('node:net').createConnection('/bridge/seat.sock');const timer=setTimeout(()=>process.exit(3),5000);s.on('connect',()=>s.write(JSON.stringify({type:'abort',commandId:process.argv[1]})+'\\n'));s.on('data',b=>{clearTimeout(timer);process.stdout.write(b);s.end()});s.on('error',e=>{console.error(e.message);process.exitCode=2});",
        commandId], { stdio: ["ignore", "pipe", "pipe"] });
      let out = "";
      let err = "";
      child.stdout.on("data", (chunk) => { out += chunk; });
      child.stderr.on("data", (chunk) => { err += chunk; });
      const exitCode = await new Promise((resolve, reject) => {
        child.once("error", reject);
        child.once("exit", resolve);
      });
      let response;
      try { response = JSON.parse(out.trim()); } catch { throw new Error(`Unauthorized abort produced invalid bridge response: ${out} ${err}`); }
      if (exitCode !== 0 || response.type !== "error" || !/authorization/.test(response.error)
        || existsSync(abortIntentPath) || !currentDispatch)
        throw new Error(`Worker-initiated abort was not rejected: ${out} ${err}`);
      record("worker_abort_denied", { container: name, commandId, response: response.error });
    },
    async destroy() {
      const cgroupLocation = cgroupPathFor(name);
      const dockerState = JSON.parse(run("docker", ["inspect", "--format", "{{json .}}", name]));
      const mounts = dockerState.Mounts;
      if (dockerState.HostConfig?.ReadonlyRootfs !== true || !dockerState.HostConfig?.CapDrop?.includes("ALL")
        || dockerState.HostConfig?.PidsLimit !== 256 || !dockerState.HostConfig?.SecurityOpt?.some((value) => value.includes("no-new-privileges"))
        || dockerState.HostConfig?.NetworkMode !== network) throw new Error("Container isolation policy differs from the qualification contract");
      const bindMappings = new Map([
        [spec.workspace, "/workspace"], [spec.bridge, "/bridge"],
        [spec.journal, "/workspace/.home/bridge.jsonl"],
        [spec.receiptDir, "/receipt"],
        [runtime.herdr, "/usr/local/bin/herdr"], [runtime.omp, "/usr/local/bin/omp"],
        [runtime.addon, "/usr/local/bin/pi_natives.linux-x64-baseline.node"],
        [runtime.node, "/usr/local/bin/node"],
        [path.resolve(path.dirname(SELF), "m1-herdr-bridge.mjs"), "/usr/local/bin/m1-herdr-bridge.mjs"],
        [runtime.peerAuth, "/usr/local/bin/m1-receipt-peer"],
      ]);
      for (const mount of mounts) {
        if (mount.Type === "bind") {
          if (bindMappings.get(mount.Source) !== mount.Destination) throw new Error(`Unexpected worker bind mount ${mount.Source} -> ${mount.Destination}`);
          const writable = mount.Source === spec.workspace || mount.Source === spec.bridge;
          if (mount.RW !== writable) throw new Error(`Worker bind mount access mode mismatch: ${mount.Destination}`);
        } else throw new Error(`Unexpected worker mount type ${mount.Type}`);
      }
      const expectedTmpfs = new Set(["/tmp", "/run", "/home/worker"]);
      const tmpfs = dockerState.HostConfig?.Tmpfs ?? {};
      if (Object.keys(tmpfs).length !== expectedTmpfs.size
        || Object.keys(tmpfs).some((destination) => !expectedTmpfs.has(destination)))
        throw new Error("Worker tmpfs layout differs from the pinned isolation layout");
      if (mounts.length !== bindMappings.size) throw new Error("Worker bind mount count does not match the pinned isolation layout");
      const journalMount = mounts.find((mount) => mount.Destination === "/workspace/.home/bridge.jsonl");
      if (!journalMount || journalMount.Source !== spec.journal || journalMount.RW !== false) throw new Error("Worker journal is not the controller-owned read-only bind");
      record("worker_mounts_verified", { container: name, mounts });
      const workerSocket = bridgeSocket;
      if (!workerSocket) throw new Error("Controller socket was already disconnected before worker exit");
      const socketClosed = new Promise((resolve) => workerSocket.once("close", () => resolve(true)));
      const begin = monotonicMs();
      run("docker", ["kill", "--signal", "TERM", name]);
      const stopDeadline = begin + 2_000;
      let state;
      while (monotonicMs() < stopDeadline) {
        state = JSON.parse(run("docker", ["inspect", "--format", "{{json .State}}", name]));
        if (!state.Running) break;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      let forcedKill = false;
      if (!state || state.Running) {
        run("docker", ["kill", "--signal", "KILL", name]);
        forcedKill = true;
      }
      const exitDeadline = monotonicMs() + 5_000;
      do {
        state = JSON.parse(run("docker", ["inspect", "--format", "{{json .State}}", name]));
        if (!state.Running) break;
        await new Promise((resolve) => setTimeout(resolve, 50));
      } while (monotonicMs() < exitDeadline);
      if (state.Running) throw new Error(`Container failed to stop after forced kill: ${name}`);
      if (ignoreStop && !forcedKill) throw new Error("SIGTERM-ignoring container did not require and receive force kill");
      if (forcedKill && state.ExitCode !== 137) throw new Error(`Force-killed container exit status was ${state.ExitCode}, expected 137`);
      const cgroup = cgroupSnapshot(name, cgroupLocation, state);
      let closeTimeout;
      const bridgeConnectionClosed = await Promise.race([socketClosed, new Promise((resolve) => { closeTimeout = setTimeout(() => resolve(false), 5_000); })]);
      clearTimeout(closeTimeout);
      if (!bridgeConnectionClosed) throw new Error("Herdr bridge connection did not close with worker/container exit");
      const containerId = containerIds.get(name);
      if (!containerId || !egressPolicies.has(name)) throw new Error("Missing container identity or egress policy before removal");
      const policyRemoval = retireSeatPolicy(name, containerId);
      run("docker", ["rm", name]);
      containers.delete(name);
      containerIds.delete(name);
      const removed = spawnSync("docker", ["inspect", containerId], { encoding: "utf8" });
      if (removed.status === 0) throw new Error("Docker container still exists after mount removal");
      run("docker", ["network", "rm", network]);
      networks.delete(network);
      const staleEndpoint = existsSync(socketPath);
      if (staleEndpoint) {
        if (!lstatSync(socketPath).isSocket()) throw new Error("Bridge endpoint changed to a non-socket after worker exit");
        unlinkSync(socketPath);
      }
      if (existsSync(socketPath)) throw new Error("Bridge socket remained after worker mount removal");
      const mountRemoval = { containerId, dockerInspectAbsent: removed.status !== 0, mountSources: [spec.workspace, spec.bridge], staleEndpointRemoved: staleEndpoint };
      const receipt = [...receiptDaemons].find((item) => item.path === receiptPath);
      if (receipt) {
        for (const socket of receipt.sockets) socket.destroy();
        await new Promise((resolve) => receipt.server.close(resolve));
        if (existsSync(receipt.path)) unlinkSync(receipt.path);
        closeSync(receipt.fd);
        receiptDaemons.delete(receipt);
      }
      const evidenceCopy = path.join(evidenceRoot, path.basename(spec.journal));
      copyFileSync(spec.journal, evidenceCopy);
      fsyncFile(evidenceCopy);
      syncDir(evidenceRoot);
      record("worker_mounts_removed", { container: name, ...mountRemoval, egressPolicyRemoved: policyRemoval.removed, bridgeConnectionClosed });
      return { cgroup, mountRemoval, bridgeConnectionClosed, forcedKill, exitCode: state.ExitCode, stopMs: monotonicMs() - begin };
    },
  };
}

function saveEvidence() {
  try { fsyncSync(journalFd); syncDir(evidenceRoot); syncDir(root); } catch (e) { throw new Error(`Evidence fsync failed: ${e.message}`); }
}

function preserveEvidence() {
  const state = path.join(os.homedir(), ".local", "state", "capstan", "m1-herdr-qualification");
  mkdirSync(state, { recursive: true, mode: 0o700 });
  const dest = path.join(state, path.basename(root));
  mkdirSync(dest, { mode: 0o700 });
  for (const name of readdirSync(evidenceRoot)) {
    const source = path.join(evidenceRoot, name);
    const destination = path.join(dest, name);
    if (!lstatSync(source).isFile()) throw new Error(`Non-file evidence entry ${name}`);
    copyFileSync(source, destination);
    fsyncFile(destination);
  }
  syncDir(dest);
  syncDir(state);
  return dest;
}

async function main() {
  mkdirSync(controllerRoot, { recursive: true, mode: 0o700 });
  if (!existsSync(EGRESS_HELPER)) throw new Error("Selected egress firewall/proxy helper is unavailable");
  const stagedHelper = path.join(controllerRoot, "m1-egress-helper.mjs");
  const stagedTls = path.join(controllerRoot, "m1-egress-tls.mjs");
  copyFileSync(EGRESS_HELPER, stagedHelper);
  copyFileSync(path.join(path.dirname(SELF), "m1-egress-tls.mjs"), stagedTls);
  chmodSync(stagedHelper, 0o600);
  chmodSync(stagedTls, 0o600);
  selectedEgressHelper = stagedHelper;
  record("egress_helper_staged", { helperSha256: sha256(readFileSync(stagedHelper)), tlsSha256: sha256(readFileSync(stagedTls)), path: stagedHelper });
  const runtime = ensureRuntime();
  runtime.node = executable(process.env.M1_NODE_BIN ?? process.execPath, "Node");
  if (run(runtime.node, ["--version"]) !== NODE_VERSION) throw new Error(`Worker Node must be ${NODE_VERSION}`);
  const peerSource = path.join(path.dirname(SELF), "m1-receipt-peer.c");
  const peerBinary = path.join(controllerRoot, "receipt-peer");
  run("cc", ["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", peerSource, "-o", peerBinary]);
  chmodSync(peerBinary, 0o755);
  runtime.peerAuth = peerBinary;
  const egressKillSource = path.join(path.dirname(SELF), "m1-egress-kill.c");
  const egressKillBinary = path.join(controllerRoot, "egress-kill");
  run("cc", ["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", egressKillSource, "-o", egressKillBinary]);
  chmodSync(egressKillBinary, 0o700);
  process.env.M1_EGRESS_KILLER = egressKillBinary;
  record("egress_pidfd_verifier_compiled", { sourceSha256: sha256(readFileSync(egressKillSource)), binarySha256: sha256(readFileSync(egressKillBinary)) });
  record("receipt_peer_auth_verified", { sourceSha256: sha256(readFileSync(peerSource)), binarySha256: sha256(readFileSync(peerBinary)),
    compiler: run("cc", ["--version"]).split("\n")[0] });
  if (!process.env.M1_PROVIDER_HOST || !/^[a-z0-9.-]+$/i.test(process.env.M1_PROVIDER_HOST)) throw new Error("M1_PROVIDER_HOST must name the exact predeclared provider host");
  record("runtime_binary_digests", { nodeSha256: sha256(readFileSync(runtime.node)), herdrSha256: sha256(readFileSync(runtime.herdr)), ompSha256: sha256(readFileSync(runtime.omp)), addonSha256: sha256(readFileSync(runtime.addon)), bridgeSha256: sha256(readFileSync(path.join(path.dirname(SELF), "m1-herdr-bridge.mjs"))), egressHelperSha256: sha256(readFileSync(stagedHelper)), egressTlsSha256: sha256(readFileSync(stagedTls)) });
  record("run_policy_frozen", {
    image: IMAGE, node: NODE_VERSION, omp: OMP_VERSION, herdr: HERDR_VERSION,
    provider: PROVIDER, providerHost: process.env.M1_PROVIDER_HOST, providerPort: process.env.M1_PROVIDER_PORT ?? "443",
    providerTokenVariable: "OPENAI_CODEX_OAUTH_TOKEN", model: MODEL,
    providerSettings: { explicitCliArgs: ["--model", MODEL], otherOverrides: [], unspecifiedSettings: "OMP 18.3.1 defaults" },
    environment: { PATH: "/usr/local/bin:/usr/bin:/bin", HOME: "/home/worker", LC_ALL: "C", LANG: "C", TZ: "UTC", TMPDIR: "/tmp", umask: "077" },
    samples: { acknowledgement: ACK_COUNT, bashProgress: PROGRESS_COUNT, replacement: REPLACEMENT_COUNT },
    limits: LIMITS,
  });
  const tornPath = path.join(controllerRoot, "torn-tail-probe.jsonl");
  const acceptedProbe = { sequence: 1, role: "PM", type: "accepted", commandId: "torn-probe",
    assignmentId: "torn-assignment", attempt: 1, generation: 1, prompt: "probe" };
  const tornFd = openSync(tornPath, "wx", 0o600);
  try {
    const bytes = Buffer.from(`${JSON.stringify(acceptedProbe)}\n{\"sequence\":2`);
    writeSync(tornFd, bytes);
    fsyncSync(tornFd);
  } finally { closeSync(tornFd); }
  const recoveredProbe = replayReceiptState(recoverControllerJournal(tornPath, "PM"));
  if (recoveredProbe.sequence !== 1 || recoveredProbe.active?.commandId !== "torn-probe"
    || readFileSync(tornPath, "utf8") !== `${JSON.stringify(acceptedProbe)}\n`)
    throw new Error("Controller failed to repair torn receipt tail before resuming append");
  record("torn_receipt_recovery_proven", { repairedSequence: recoveredProbe.sequence, survivingSha256: sha256(readFileSync(tornPath)) });
  unlinkSync(tornPath);
  syncDir(controllerRoot);
  const runs = { ack: [], progress: [], replacements: [] };

  // Ack probes measure launch through the receiver's fsynced identity acknowledgement.
  for (let i = 0; i < ACK_COUNT; i++) {
    const role = ROLES[i % ROLES.length];
    const spec = makeWorkspace(role, `ack-${String(i + 1).padStart(2, "0")}`);
    const seat = await createSeat(runtime, spec);
    const id = `m1-ack-${i + 1}`;
    const prompt = "Reply exactly M1_ACK. Do not use tools.";
    const dispatch = { type: "dispatch", commandId: id, assignmentId: `m1-assignment-${id}`, attempt: 1, generation: 1, prompt };
    const command = await seat.command(dispatch);
    runs.ack.push(command.ackMs);
    const result = await seat.completed(id, LIMITS.ackMs);
    if (result.reply !== "M1_ACK") throw new Error(`Acknowledgement probe got unexpected durable reply: ${result.reply}`);
    const provider = seat.verifyProvider();
    record("ack_completion", { id, role, reply: result.reply, evidenceRef: result.evidenceRef, provider });
    if (i === 0) {
      await seat.reconnectController();
      const restarted = independentReconcileClientGet(path.join(spec.bridge, "seat.sock"), id);
      if (JSON.stringify(restarted.evidenceRef) !== JSON.stringify(result.evidenceRef) || restarted.reply !== result.reply) throw new Error("Fresh reconciliation client did not read exact completion");
      verifyCompletionFrame(restarted, spec);
      const duplicate = await seat.reconcileDuplicate(dispatch, result.evidenceRef);
      record("fresh_client_duplicate_reconciled", { id, freshClientEvidenceRef: restarted.evidenceRef, duplicateEvidenceRef: duplicate.result.evidenceRef, replayEvidenceRef: duplicate.replay.evidenceRef });
    }
    const cleanup = await seat.destroy();
    record("ack_probe_cleaned", { id, cleanup });
    if (i === 0) {
      const forged = createServer((socket) => {
        socket.on("error", () => {});
        socket.end(`${JSON.stringify({ type: "completed", commandId: id, reply: "FAKE", evidenceRef: result.evidenceRef })}\n`);
      });
      await new Promise((resolve, reject) => { forged.once("error", reject); forged.listen(path.join(spec.bridge, "seat.sock"), resolve); });
      try {
        let rejected = false;
        try { await seat.reconnectController(1_000); } catch (error) { rejected = /Bridge server peer/.test(error.message); }
        if (!rejected) throw new Error("Controller accepted a worker-replaced bridge socket");
        record("forged_bridge_socket_rejected", { id });
      } finally {
        await new Promise((resolve) => forged.close(resolve));
      }
    }
  }

  // Long-running declared bash actions must emit advancing, command-correlated durable observations.
  for (let i = 0; i < PROGRESS_COUNT; i++) {
    const role = ROLES[i % ROLES.length];
    const spec = makeWorkspace(role, `progress-${String(i + 1).padStart(2, "0")}`);
    const seat = await createSeat(runtime, spec);
    const id = `m1-progress-${i + 1}`;
    const prompt = "Use the bash tool for exactly one foreground POSIX shell command in /workspace: printf '1\\n' >> progress.jsonl; sleep 10; printf '2\\n' >> progress.jsonl; sleep 10; printf '3\\n' >> progress.jsonl. Do not use Python or another interpreter. Reply exactly M1_PROGRESS_DONE.";
    const assignmentId = `m1-assignment-${id}`;
    const dispatch = { type: "dispatch", commandId: id, assignmentId, attempt: 1, generation: 1, prompt };
    const { ack } = await seat.command(dispatch);
    const ackAt = monotonicMs();
    const qualifying = [];
    const seenEvidence = new Set();
    let activeToolCall = null;
    while (true) {
      const event = await seat.lifecycle(id, ["tool_started", "tool_completed", "completed"], LIMITS.progressMs);
      if (event.assignmentId !== assignmentId || event.attempt !== 1 || event.generation !== 1) throw new Error(`Progress event identity mismatch: ${JSON.stringify(event)}`);
      if (!event.evidenceRef || typeof event.evidenceRef.journal !== "string" || !Number.isSafeInteger(event.evidenceRef.sequence)) throw new Error(`Progress event lacks immutable evidence identity: ${JSON.stringify(event)}`);
      if (event.type === "completed") verifyCompletionFrame(event, spec);
      else {
        if (event.evidenceRef.journal !== "/workspace/.home/bridge.jsonl" || event.evidenceRef.sequence < 1)
          throw new Error("Progress tool receipt is not in the controller-owned journal");
        const entry = recoverControllerJournal(spec.journal, role)[event.evidenceRef.sequence - 1];
        if (!entry || entry.sequence !== event.evidenceRef.sequence || entry.type !== event.type
          || entry.commandId !== id || entry.assignmentId !== assignmentId || entry.attempt !== 1 || entry.generation !== 1
          || entry.toolName !== event.toolName || entry.toolCallId !== event.toolCallId
          || JSON.stringify(entry.evidenceRef) !== JSON.stringify(event.evidenceRef))
          throw new Error("Progress tool event does not match fsynced controller journal");
      }
      const ref = `${event.evidenceRef.journal}:${event.evidenceRef.sequence}`;
      if (seenEvidence.has(ref)) throw new Error(`Progress event reused evidence identity ${ref}`);
      seenEvidence.add(ref);
      if (event.type === "tool_started") {
        if (activeToolCall || event.toolName !== "bash" || typeof event.toolCallId !== "string") throw new Error("Progress probe did not start exactly one declared bash action");
        activeToolCall = event.toolCallId;
      } else if (event.type === "tool_completed") {
        if (!activeToolCall || event.toolName !== "bash" || event.toolCallId !== activeToolCall || event.isError === true) throw new Error("Progress completion did not confirm a successful active bash action");
      } else if (!activeToolCall || event.reply !== "M1_PROGRESS_DONE") {
        throw new Error("Progress probe did not finish with the accepted exact reply");
      }
      const observedAt = event._arrivalMonotonicMs ?? monotonicMs();
      const observation = { at: observedAt, type: event.type, toolName: event.toolName, toolCallId: event.toolCallId, evidenceRef: event.evidenceRef };
      qualifying.push(observation);
      record("progress_observation", { id, role, observation });
      if (i === 0 && event.type === "tool_started") await seat.restartReceiptDaemon();
      if (event.type === "completed") break;
    }
    if (qualifying.map((x) => x.type).join(",") !== "tool_started,tool_completed,completed") throw new Error("Progress lifecycle was incomplete or included non-advancing events");
    const progressBytes = readFileSync(path.join(spec.workspace, "progress.jsonl"), "utf8");
    if (progressBytes !== "1\n2\n3\n") throw new Error("Declared bash work counter did not advance exactly through the three requested milestones");
    const progressManifest = scanTree(spec.workspace);
    record("progress_manifest", { id, sha256: progressManifest.sha256, entries: progressManifest.entries });
    const points = [ackAt, ...qualifying.map((x) => x.at)];
    const silenceIntervals = points.slice(1).map((point, j) => Math.max(0, point - points[j]));
    const silence = Math.max(...silenceIntervals);
    if (silence > LIMITS.progressMs) throw new Error(`Progress silence exceeded 900s (${silence}ms)`);
    runs.progress.push(silence);
    const provider = seat.verifyProvider();
    record("progress_sample", { id, role, silenceMs: silence, silenceIntervalsMs: silenceIntervals, observationTypes: qualifying.map((x) => x.type), ackState: ack.state, provider });
    const cleanup = await seat.destroy();
    record("progress_probe_cleaned", { id, cleanup });
  }

  // Replacement records include ten independently launched writers; one deliberately ignores TERM.
  const retiredWorkspaces = [];
  for (let i = 0; i < REPLACEMENT_COUNT; i++) {
    const role = ROLES[i % ROLES.length];
    const spec = makeWorkspace(role, `replacement-${String(i + 1).padStart(2, "0")}`);
    const seat = await createSeat(runtime, spec, { ignoreStop: i === 0 });
    const id = `m1-replace-${i + 1}`;
    const assignmentId = `m1-assignment-${id}`;
    const prompt = "Use the bash tool to run this bounded foreground command and do not return before it starts: trap '' TERM; for i in $(seq 1 300); do printf x >> /workspace/writer.log; sleep 0.1; done. This loop writes at most 300 bytes and exits after at most 30 seconds; its TERM-ignoring process must be contained before restoring the workspace.";
    const dispatch = { type: "dispatch", commandId: id, assignmentId, attempt: 1, generation: 1, prompt };
    await seat.command(dispatch);
    const started = await seat.lifecycle(id, ["tool_started"], 60_000);
    if (started.assignmentId !== assignmentId || started.attempt !== 1 || started.generation !== 1 || started.toolName !== "bash" || typeof started.toolCallId !== "string" || !started.evidenceRef) {
      throw new Error("Replacement writer did not produce qualifying correlated bash-start evidence");
    }
    const writer = path.join(spec.workspace, "writer.log");
    const writerDeadline = monotonicMs() + 15_000;
    while (!existsSync(writer) && monotonicMs() < writerDeadline) await new Promise((resolve) => setTimeout(resolve, 50));
    if (!existsSync(writer) || statSync(writer).size === 0) throw new Error("Replacement writer never mutated its assigned workspace");
    if (i === 0) await seat.probeUnauthorizedAbort(id);
    const revocation = monotonicMs();
    const aborted = await seat.abort(id);
    if (aborted.type !== "ack" || aborted.durable !== true || aborted.state !== "unknown") throw new Error("Bridge did not durably revoke the active assignment capability");
    record("capability_revoked", { id, role, assignmentId, atMonotonicMs: revocation, abort: aborted });
    const cleanup = await seat.destroy();
    const stoppedManifest = scanTree(spec.workspace);
    if (!stoppedManifest.entries.some((entry) => entry.path === "writer.log" && entry.type === "file" && entry.size > 0)) throw new Error("No-follow scan did not find writer evidence after worker exit");
    const stoppedWriterBytes = readFileSync(path.join(spec.workspace, "writer.log"), "utf8");
    const elapsed = monotonicMs() - revocation;
    if (elapsed > LIMITS.replacementMs) throw new Error(`Replacement exceeded 30s (${elapsed}ms)`);
    if (i === 0 && (!cleanup.forcedKill || cleanup.exitCode !== 137)) throw new Error("TERM-ignoring Herdr container was not force-killed");
    runs.replacements.push(elapsed);
    const retiredWorkspace = { id, workspace: spec.workspace };
    retiredWorkspaces.push(retiredWorkspace);
    record("replacement_sample", { id, role, milliseconds: elapsed, stoppedManifestSha256: stoppedManifest.sha256, cgroupEmpty: cleanup.cgroup, mountRemoval: cleanup.mountRemoval, forcedKill: cleanup.forcedKill, exitCode: cleanup.exitCode });

    const intervalStart = monotonicMs();
    await new Promise((resolve) => setTimeout(resolve, 30_000));
    const stable = scanTree(spec.workspace);
    if (stable.sha256 !== stoppedManifest.sha256) throw new Error(`Old writer mutated workspace during pre-replacement quiescence ${id}`);
    const quiescenceObservedMs = monotonicMs() - intervalStart;
    record("pre_replacement_quiescence", { id, observedMs: quiescenceObservedMs, oldManifestSha256: stable.sha256 });

    const replacementSpec = makeWorkspace(role, `replacement-next-${String(i + 1).padStart(2, "0")}`, spec.workspace);
    const replacement = await createSeat(runtime, replacementSpec);
    const replacementId = `${id}-restore`;
    const replacementAssignment = `m1-assignment-${replacementId}`;
    const replacementPrompt = "Use a tool to write exactly M1_REPLACEMENT_READY followed by a newline to /workspace/replacement.started, then reply exactly M1_REPLACEMENT_READY.";
    const replacementDispatch = { type: "dispatch", commandId: replacementId, assignmentId: replacementAssignment, attempt: 1, generation: 1, prompt: replacementPrompt };
    await replacement.command(replacementDispatch);
    const restoredStart = await replacement.lifecycle(replacementId, ["tool_started"], 60_000);
    const restoredTool = await replacement.lifecycle(replacementId, ["tool_completed"], 60_000);
    const restoredResult = await replacement.completed(replacementId, 60_000);
    if (restoredStart.assignmentId !== replacementAssignment || restoredStart.attempt !== 1 || restoredStart.generation !== 1
      || typeof restoredStart.toolName !== "string" || !restoredStart.toolName || !restoredStart.evidenceRef
      || restoredTool.assignmentId !== replacementAssignment || restoredTool.attempt !== 1 || restoredTool.generation !== 1
      || restoredTool.toolCallId !== restoredStart.toolCallId || restoredTool.toolName !== restoredStart.toolName
      || restoredTool.isError === true || !restoredTool.evidenceRef
      || JSON.stringify(restoredStart.evidenceRef) === JSON.stringify(restoredTool.evidenceRef)
      || restoredResult.reply !== "M1_REPLACEMENT_READY") throw new Error("Replacement Herdr seat did not complete correlated restoration action");
    if (readFileSync(path.join(spec.workspace, "writer.log"), "utf8") !== stoppedWriterBytes) throw new Error("Retired writer mutated its workspace during replacement");
    if (readFileSync(path.join(spec.workspace, "replacement.started"), "utf8") !== "M1_REPLACEMENT_READY\n") throw new Error("Replacement did not restore onto the revoked writer's workspace with exact bytes");
    const replacementManifest = scanTree(replacementSpec.workspace);
    const writerEntry = replacementManifest.entries.find((entry) => entry.path === "writer.log" && entry.type === "file");
    const restoreEntry = replacementManifest.entries.find((entry) => entry.path === "replacement.started" && entry.type === "file");
    if (!writerEntry?.size || !restoreEntry) throw new Error("Restored workspace is missing prior writer or replacement action evidence");
    retiredWorkspace.writerLogSha256 = writerEntry.sha256;
    retiredWorkspace.restorationSha256 = restoreEntry.sha256;
    record("replacement_manifest", { id: replacementId, sha256: replacementManifest.sha256, entries: replacementManifest.entries });
    replacement.verifyProvider();
    const ordering = { capabilityRevokedAtMonotonicMs: revocation, exitCode: cleanup.exitCode, cgroupEmpty: cleanup.cgroup.pids === 0, bridgeConnectionClosed: cleanup.bridgeConnectionClosed, staleEndpointRemoved: cleanup.mountRemoval.staleEndpointRemoved, replacementControllerConnectedAtMonotonicMs: replacement.controllerConnectedAt };
    if (!ordering.cgroupEmpty || !ordering.bridgeConnectionClosed || !(ordering.replacementControllerConnectedAtMonotonicMs > revocation)) throw new Error("Exit/close/restore ordering proof incomplete");
    const replacementCleanup = await replacement.destroy();
    record("replacement_writer_exclusion", { id, replacementId, quiescenceObservedMs, oldManifestSha256: stable.sha256, ordering, replacementCleanup });
  }

  const max = (samples) => Math.max(...samples);
  const quiescenceMs = Math.min(30_000, Math.max(5_000, 3 * max(runs.replacements)));
  const ackDeadlineMs = Math.min(120_000, Math.max(30_000, 3 * max(runs.ack)));
  const progressWindowMs = Math.min(900_000, Math.max(120_000, 3 * max(runs.progress)));
  if (runs.ack.length !== ACK_COUNT || runs.progress.length !== PROGRESS_COUNT || runs.replacements.length !== REPLACEMENT_COUNT || retiredWorkspaces.length !== REPLACEMENT_COUNT) {
    throw new Error("Frozen M1 sample cardinality failure");
  }
  const quiescenceStart = monotonicMs();
  await new Promise((resolve) => setTimeout(resolve, quiescenceMs));
  for (const retired of retiredWorkspaces) {
    const manifest = scanTree(retired.workspace);
    const entries = new Map(manifest.entries.map((entry) => [entry.path, entry]));
    if (entries.get("writer.log")?.sha256 !== retired.writerLogSha256
      || entries.get("replacement.started")?.sha256 !== retired.restorationSha256)
      throw new Error(`Old writer or restoration output changed during selected quiescence interval: ${retired.id}`);
    record("quiescence_manifest", { id: retired.id, intervalMs: monotonicMs() - quiescenceStart, manifestSha256: manifest.sha256, writerLogSha256: retired.writerLogSha256, restorationSha256: retired.restorationSha256, stable: true });
  }
  const quiescenceObservedMs = monotonicMs() - quiescenceStart;
  if (quiescenceObservedMs < quiescenceMs) throw new Error("Selected quiescence interval was censored");
  record("qualification_values_frozen", { ackRawMs: runs.ack, progressSilenceRawMs: runs.progress, replacementRawMs: runs.replacements, quiescenceMs, ackDeadlineMs, progressWindowMs, quiescenceObservedMs });
  saveEvidence();
  return { result: "PASS", versions: { herdr: HERDR_VERSION, omp: OMP_VERSION, node: NODE_VERSION, image: IMAGE }, samples: { ackMs: runs.ack, progressSilenceMs: runs.progress, replacementMs: runs.replacements }, selected: { quiescenceMs, ackDeadlineMs, progressWindowMs }, evidenceRoot: root };
}

async function finish() {
  let durableEvidence;
  const cleanupErrors = [];
  try {
    summary = await main();
  } catch (error) {
    failed = true;
    terminalError = error instanceof Error ? error.stack ?? error.message : String(error);
    try { record("qualification_failed", { reason: terminalError }); } catch {}
  } finally {
    for (const name of [...containers]) {
      if (failed) {
        for (const [label, args] of [
          ["docker", ["logs", name]],
          ["pane", ["exec", name, "/usr/local/bin/herdr", "pane", "read", diagnosticPanes.get(name) ?? "w1:p1", "--source", "recent-unwrapped", "--lines", "120"]],
        ]) {
          try {
            const diagnostic = spawnSync("docker", args, { encoding: "utf8", timeout: 5_000, maxBuffer: 1_000_000 });
            const bytes = Buffer.from(`${diagnostic.stdout ?? ""}\n${diagnostic.stderr ?? ""}`);
            const file = path.join(evidenceRoot, `${name}.${label}.txt`);
            const fd = openSync(file, "w", 0o600);
            try {
              for (let offset = 0; offset < bytes.length;) {
                const written = writeSync(fd, bytes, offset, bytes.length - offset);
                if (written <= 0) throw new Error("Diagnostic write made no progress");
                offset += written;
              }
              fsyncSync(fd);
            } finally { closeSync(fd); }
            record("diagnostic_preserved", { name, label, sha256: sha256(bytes), path: file });
          } catch (error) { cleanupErrors.push(`diagnostic ${name}/${label}: ${error.message}`); }
        }
      }
      const id = containerIds.get(name);
      if (!id) { cleanupErrors.push(`container ${name}: no durable container ID for policy cleanup`); continue; }
      try {
        retireSeatPolicy(name, id);
        run("docker", ["rm", "-f", name]);
        containers.delete(name);
        containerIds.delete(name);
      } catch (error) {
        record("container_cleanup_deferred", { name, reason: error.message });
      }
    }
    for (let attempt = 0; containers.size && attempt < 30; attempt++) {
      for (const name of [...containers]) {
        try {
          const id = containerIds.get(name);
          if (!id) throw new Error("missing container identity");
          let state = JSON.parse(run("docker", ["inspect", "--format", "{{json .State}}", name]));
          if (state.Running) {
            try { run("docker", ["kill", "--signal", "KILL", name]); } catch {}
            state = JSON.parse(run("docker", ["inspect", "--format", "{{json .State}}", name]));
          }
          if (state.Running) continue;
          retireSeatPolicy(name, id);
          run("docker", ["rm", "-f", name]);
          containers.delete(name);
          containerIds.delete(name);
          record("container_emergency_containment_complete", { name, id, attempt: attempt + 1 });
        } catch (error) {
          record("container_emergency_containment_retry", { name, attempt: attempt + 1, reason: error.message });
          try { saveEvidence(); } catch {}
        }
      }
      if (containers.size && attempt < 29) await new Promise((resolve) => setTimeout(resolve, 1_000));
    }
    for (const name of containers) cleanupErrors.push(`container ${name} remains after bounded emergency containment retries`);
    for (const [name, policy] of [...egressPolicies]) {
      if (containers.has(name)) continue;
      try {
        const result = egress("cleanup-container", ["--container", policy.containerId]);
        if (result.removed !== true) throw new Error("helper did not confirm policy removal");
        egressPolicies.delete(name);
      } catch (error) { cleanupErrors.push(`orphan egress policy ${policy.policyId}: ${error.message}`); }
    }
    for (const network of [...networks]) {
      try { run("docker", ["network", "rm", network]); networks.delete(network); }
      catch (error) { cleanupErrors.push(`network ${network}: ${error.message}`); }
    }
    for (const receipt of [...receiptDaemons]) {
      try {
        for (const socket of receipt.sockets) socket.destroy();
        await new Promise((resolve) => receipt.server.close(resolve));
        closeSync(receipt.fd);
        try { unlinkSync(receipt.path); } catch (error) { if (error?.code !== "ENOENT") throw error; }
        const evidenceCopy = path.join(evidenceRoot, path.basename(receipt.journal));
        copyFileSync(receipt.journal, evidenceCopy);
        fsyncFile(evidenceCopy);
        syncDir(evidenceRoot);
        receiptDaemons.delete(receipt);
      } catch (error) { cleanupErrors.push(`receipt daemon ${receipt.path}: ${error.message}`); }
    }
    if (cleanupErrors.length) {
      failed = true;
      terminalError = `${terminalError ?? ""}\nCleanup incomplete: ${cleanupErrors.join("; ")}`.trim();
    }
    try {
      record("cleanup", { status: failed ? "failed" : "complete", containersRemaining: [...containers], policiesRemaining: [...egressPolicies.keys()], networksRemaining: [...networks], cleanupErrors });
      saveEvidence();
      closeSync(journalFd);
      durableEvidence = preserveEvidence();
    } catch (error) {
      failed = true;
      terminalError = `${terminalError ?? ""}\nEvidence preservation failed: ${error.message}`.trim();
    }
    if (!failed) {
      try { rmSync(root, { recursive: true }); }
      catch (error) { failed = true; terminalError = `Successful run workspace cleanup failed: ${error.message}`; }
    }
    if (summary && durableEvidence) summary.evidenceRoot = durableEvidence;
  }
  if (failed) {
    console.error(JSON.stringify({ result: "FAIL", reason: terminalError, evidenceRoot: durableEvidence ?? root, workspaceRoot: root, preserved: existsSync(journalPath), cleanupErrors }));
    process.exitCode = 1;
  } else console.log(JSON.stringify(summary));
}
if (process.argv[1] === SELF) await finish();
export { scanTree };

#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { lookup } from "node:dns/promises";
import { closeSync, existsSync, fsyncSync, mkdirSync, openSync, readFileSync, rmSync, writeSync } from "node:fs";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { sniFromHello } from "./m1-egress-tls.mjs";

const SELF = fileURLToPath(import.meta.url);
const DIR = path.join(os.tmpdir(), `capstan-m1-egress-${process.getuid()}`);
const IPTABLES = "/usr/sbin/iptables";
const MAX_HELLO = 65_536;

function option(name) {
  const index = process.argv.indexOf(`--${name}`);
  if (index < 0 || !process.argv[index + 1]) throw new Error(`Missing --${name}`);
  return process.argv[index + 1];
}
function validPort(value, allowZero = false) {
  const number = Number(value);
  if (!Number.isInteger(number) || number < (allowZero ? 0 : 1) || number > 65535) throw new Error(`Invalid port ${value}`);
  return number;
}
function validatedContainer() {
  const id = option("container");
  if (!/^[0-9a-f]{64}$/.test(id)) throw new Error("Expected a full Docker container ID");
  return id;
}
function location(id) { return path.join(DIR, `${id}.json`); }
function docker(...args) {
  const result = spawnSync("docker", args, { encoding: "utf8", timeout: 15_000 });
  if (result.status !== 0) throw new Error(`Docker ${args[0]} failed: ${result.stderr.trim()}`);
  return result.stdout.trim();
}
function firewall(...args) {
  const result = spawnSync("sudo", ["-n", IPTABLES, ...args], { encoding: "utf8", timeout: 10_000 });
  if (result.status !== 0) throw new Error(`Firewall ${args[0]} failed: ${result.stderr.trim()}`);
  return result.stdout;
}
function acceptArgs(state) {
  return ["DOCKER-USER", "-s", state.containerIp, "-d", state.gateway, "-p", "tcp", "--dport", String(state.port), "-m", "comment", "--comment", state.policyId, "-j", "ACCEPT"];
}
function rejectArgs(state) {
  return ["DOCKER-USER", "-s", state.containerIp, "-m", "comment", "--comment", state.policyId, "-j", "REJECT"];
}
function inputAcceptArgs(state) {
  return ["INPUT", "-s", state.containerIp, "-d", state.gateway, "-p", "tcp", "--dport", String(state.port), "-m", "comment", "--comment", state.policyId, "-j", "ACCEPT"];
}
function inputRejectArgs(state) {
  return ["INPUT", "-s", state.containerIp, "-m", "comment", "--comment", state.policyId, "-j", "REJECT"];
}
function hasRule(args) {
  const result = spawnSync("sudo", ["-n", IPTABLES, "-C", ...args], { encoding: "utf8", timeout: 10_000 });
  if (result.status === 0) return true;
  if (result.status === 1 && /Bad rule|does a matching rule exist/i.test(result.stderr)) return false;
  throw new Error(`Could not check firewall rule: ${result.stderr.trim() || result.error?.message || result.status}`);
}
function insertIfAbsent(args) {
  if (!hasRule(args)) firewall("-I", args[0], "1", ...args.slice(1));
}
function deleteAll(args) {
  while (hasRule(args)) firewall("-D", ...args);
}
function installPolicy(state) {
  insertIfAbsent(rejectArgs(state));
  insertIfAbsent(acceptArgs(state));
  insertIfAbsent(inputRejectArgs(state));
  insertIfAbsent(inputAcceptArgs(state));
}
function syncFile(file, content) {
  const fd = openSync(file, "wx", 0o600);
  try {
    const bytes = Buffer.from(content);
    for (let offset = 0; offset < bytes.length;) {
      const count = writeSync(fd, bytes, offset, bytes.length - offset);
      if (count <= 0) throw new Error("Egress state write stopped");
      offset += count;
    }
    fsyncSync(fd);
  } finally { closeSync(fd); }
  const parent = openSync(DIR, "r");
  try { fsyncSync(parent); } finally { closeSync(parent); }
}
function proxyIdentity(pid, startTicks, mode) {
  const killer = process.env.M1_EGRESS_KILLER;
  if (!killer || !existsSync(killer)) throw new Error("Pidfd-safe egress proxy verifier is unavailable");
  const result = spawnSync(killer, [mode, String(pid), String(startTicks), SELF], { encoding: "utf8", timeout: mode === "--terminate" ? 12_000 : 5_000 });
  if (result.error || result.status !== 0) throw new Error(`Egress proxy process identity check failed: ${(result.stderr ?? result.error?.message ?? "").trim()}`);
}
function procStartTicks(pid) {
  const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
  const fields = stat.slice(stat.lastIndexOf(")") + 2).trim().split(/\s+/);
  const startTicks = fields[19];
  if (!/^\d+$/.test(startTicks ?? "")) throw new Error("Cannot read egress proxy process start identity");
  return startTicks;
}
function readState(id, policyId) {
  const state = JSON.parse(readFileSync(location(id), "utf8"));
  if (state.policyId !== policyId || state.container !== id) throw new Error("Egress policy identity mismatch");
  return state;
}
export function isContainerQuiesced(state) {
  return Boolean(state && (!state.Running || state.Paused));
}
function assertContainerQuiesced(container) {
  let state = JSON.parse(docker("inspect", container))[0]?.State;
  if (state?.Running && !state.Paused) throw new Error("Refusing to remove egress policy while the worker is live and unpaused");
  if (state?.Paused) {
    docker("stop", "--time", "0", container);
    state = JSON.parse(docker("inspect", container))[0]?.State;
    if (state?.Running || state?.Paused) throw new Error("Worker did not stop before egress policy removal");
  }
}
function removePolicy(state) {
  assertContainerQuiesced(state.container);
  deleteAll(inputAcceptArgs(state));
  deleteAll(inputRejectArgs(state));
  deleteAll(acceptArgs(state));
  deleteAll(rejectArgs(state));
  proxyIdentity(state.pid, state.startTicks, "--terminate");
  rmSync(location(state.container), { force: true });
  rmSync(state.audit, { force: true });
  const fd = openSync(DIR, "r");
  try { fsyncSync(fd); } finally { closeSync(fd); }
}
const NON_PUBLIC_IPV4 = [
  ["0.0.0.0", 8], ["10.0.0.0", 8], ["100.64.0.0", 10], ["127.0.0.0", 8],
  ["169.254.0.0", 16], ["172.16.0.0", 12], ["192.0.0.0", 24], ["192.0.2.0", 24],
  ["192.31.196.0", 24], ["192.52.193.0", 24], ["192.88.99.0", 24],
  ["192.168.0.0", 16], ["192.175.48.0", 24], ["198.18.0.0", 15],
  ["198.51.100.0", 24], ["203.0.113.0", 24], ["224.0.0.0", 4], ["240.0.0.0", 4],
].map(([base, bits]) => {
  const value = base.split(".").reduce((number, part) => (number * 256) + Number(part), 0) >>> 0;
  return [value, bits === 0 ? 0 : (0xffffffff << (32 - bits)) >>> 0];
});
function shellWords(line) {
  const words = [];
  let word = "";
  let quote = "";
  let escaped = false;
  let started = false;
  for (const character of line) {
    if (escaped) { word += character; escaped = false; started = true; continue; }
    if (character === "\\" && quote !== "'") { escaped = true; started = true; continue; }
    if (quote) {
      if (character === quote) quote = "";
      else word += character;
      started = true;
      continue;
    }
    if (character === "'" || character === '"') { quote = character; started = true; continue; }
    if (/\s/.test(character)) {
      if (started) words.push(word);
      word = "";
      started = false;
      continue;
    }
    word += character;
    started = true;
  }
  if (escaped || quote) throw new Error("Malformed quoted iptables rule");
  if (started) words.push(word);
  return words;
}
export function ruleTarget(line) {
  const words = shellWords(line);
  for (let index = 0; index < words.length; index++) {
    if (words[index] === "-j" || words[index] === "-g") {
      if (!words[index + 1]) throw new Error("iptables rule has a jump without a target");
      return { kind: words[index], target: words[index + 1] };
    }
  }
  throw new Error("iptables rule has no jump target");
}
export function publicIPv4(ip) {
  if (net.isIP(ip) !== 4) return false;
  const value = ip.split(".").reduce((number, part) => (number * 256) + Number(part), 0) >>> 0;
  return NON_PUBLIC_IPV4.every(([base, mask]) => ((value & mask) >>> 0) !== base);
}
export function sniMatchesHost(sni, host) {
  return sni.toLowerCase() === host.toLowerCase();
}
function appendAudit(file, record) {
  const existed = existsSync(file);
  const fd = openSync(file, "a", 0o600);
  try {
    const bytes = Buffer.from(`${JSON.stringify(record)}\n`);
    for (let offset = 0; offset < bytes.length;) {
      const count = writeSync(fd, bytes, offset, bytes.length - offset);
      if (count <= 0) throw new Error("Egress audit write stopped");
      offset += count;
    }
    fsyncSync(fd);
  } finally { closeSync(fd); }
  if (!existed) {
    const dirFd = openSync(DIR, "r");
    try { fsyncSync(dirFd); } finally { closeSync(dirFd); }
  }
}
async function serve() {
  const gateway = option("gateway");
  const port = validPort(option("proxy-port"), true);
  const host = option("provider-host").toLowerCase();
  const providerPort = validPort(option("provider-port"));
  const containerIp = option("container-ip");
  const audit = option("audit");
  const server = http.createServer((request, response) => {
    appendAudit(audit, { event: "http_denied", at: new Date().toISOString(), method: request.method });
    response.writeHead(403).end();
  });
  server.on("connect", (request, client, head) => {
    appendAudit(audit, { event: "connect_attempt", at: new Date().toISOString(), target: request.url, remote: client.remoteAddress });
    let target;
    try {
      target = new URL(`http://${request.url}`);
      if (client.remoteAddress !== containerIp || target.hostname !== host || Number(target.port) !== providerPort
        || target.username || target.password || target.pathname !== "/") throw new Error("CONNECT target denied");
    } catch (error) {
      appendAudit(audit, { event: "connect_denied", at: new Date().toISOString(), reason: error.message });
      client.destroy();
      return;
    }
    client.write("HTTP/1.1 200 Connection Established\r\n\r\n");
    const helloDeadline = setTimeout(() => {
      appendAudit(audit, { event: "tls_denied", at: new Date().toISOString(), reason: "TLS ClientHello deadline exceeded" });
      client.destroy();
    }, 10_000);
    client.once("close", () => clearTimeout(helloDeadline));
    let buffered = head;
    const receive = async (chunk) => {
      try {
        buffered = Buffer.concat([buffered, chunk]);
        if (buffered.length > MAX_HELLO) throw new Error("TLS ClientHello exceeds limit");
        const sni = sniFromHello(buffered);
        if (sni === null) return;
        if (!sniMatchesHost(sni, host)) throw new Error("TLS SNI target denied");
        clearTimeout(helloDeadline);
        client.removeListener("data", receive);
        client.pause();
        const resolved = await lookup(host, { family: 4 });
        if (!publicIPv4(resolved.address)) throw new Error("Provider DNS resolved outside public IPv4");
        const upstream = net.connect(providerPort, resolved.address);
        upstream.once("connect", () => {
          try { appendAudit(audit, { event: "provider_connect", at: new Date().toISOString(), host, port: providerPort, sni, address: resolved.address }); }
          catch { upstream.destroy(); client.destroy(); return; }
          upstream.write(buffered);
          client.pipe(upstream).pipe(client);
          client.resume();
        });
        upstream.on("error", (error) => {
          appendAudit(audit, { event: "upstream_failed", at: new Date().toISOString(), reason: error.code ?? error.message });
          client.destroy();
        });
        client.on("error", () => upstream.destroy());
      } catch (error) {
        appendAudit(audit, { event: "tls_denied", at: new Date().toISOString(), reason: error.message });
        client.destroy();
      }
    };
    client.on("data", receive);
    client.on("error", () => {});
    if (head.length) { buffered = Buffer.alloc(0); void receive(head); }
  });
  server.on("error", (error) => { console.error(error.message); process.exitCode = 1; });
  server.listen(port, gateway, () => { console.log(JSON.stringify({ port: server.address().port })); });
}
async function prepare() {
  const container = validatedContainer();
  const containerIp = option("container-ip");
  if (net.isIP(containerIp) !== 4) throw new Error("Expected container IPv4 address");
  const host = option("provider-host").toLowerCase();
  if (!/^[a-z0-9.-]+$/.test(host) || host.startsWith(".") || host.endsWith(".")) throw new Error("Invalid provider host");
  const providerPort = validPort(option("provider-port"));
  const desiredPort = validPort(option("proxy-port"), true);
  const inspect = JSON.parse(docker("inspect", container))[0];
  if (!inspect?.State?.Running) throw new Error("Container is not running");
  const networks = Object.entries(inspect.NetworkSettings.Networks);
  if (networks.length !== 1 || networks[0][1].IPAddress !== containerIp) {
    throw new Error("Container network identity mismatch");
  }
  const network = JSON.parse(docker("network", "inspect", networks[0][0]))[0];
  if (network.Internal !== true || network.EnableIPv6 !== false
    || networks[0][1].GlobalIPv6Address || networks[0][1].IPv6Gateway
    || network.IPAM?.Config?.length !== 1 || net.isIP(network.IPAM.Config[0].Gateway) !== 4) {
    throw new Error("Expected one isolated IPv4-only internal network with IPv4 gateway");
  }
  const gateway = network.IPAM.Config[0].Gateway;
  mkdirSync(DIR, { recursive: true, mode: 0o700 });
  const reservation = `${location(container)}.prepare`;
  mkdirSync(reservation, { mode: 0o700 });
  try {
    if (existsSync(location(container))) throw new Error("Existing egress policy requires explicit cleanup");
  const policyId = `capstan-m1-${randomBytes(12).toString("hex")}`;
  const audit = path.join(DIR, `${container}.events.jsonl`);
  const child = spawn(process.execPath, [SELF, "serve", "--gateway", gateway, "--proxy-port", String(desiredPort), "--provider-host", host,
    "--provider-port", String(providerPort), "--container-ip", containerIp, "--audit", audit], { detached: true, stdio: ["ignore", "pipe", "ignore"] });
  let startupTimer;
  let port;
  let startTicks;
  try {
    port = await new Promise((resolve, reject) => {
      let buffer = "";
      startupTimer = setTimeout(() => reject(new Error("Egress proxy did not start")), 5000);
      child.on("error", reject);
      child.on("exit", (code) => reject(new Error(`Egress proxy exited ${code}`)));
      child.stdout.on("data", (bytes) => { buffer += bytes.toString("utf8"); if (buffer.includes("\n")) {
        try { resolve(validPort(JSON.parse(buffer.slice(0, buffer.indexOf("\n"))).port)); } catch (error) { reject(error); }
      } });
    });
    startTicks = procStartTicks(child.pid);
  } catch (error) {
    if (child.pid && child.exitCode === null && child.signalCode === null) {
      const exited = new Promise((resolve) => child.once("exit", resolve));
      child.kill("SIGTERM");
      await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 1000))]);
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, 1000))]);
      if (child.exitCode === null && child.signalCode === null)
        throw new AggregateError([error], "Egress proxy startup failed and the listener could not be stopped");
    }
    rmSync(audit, { force: true });
    throw error;
  } finally {
    clearTimeout(startupTimer);
    child.stdout.destroy();
  }
  child.unref();
  const state = { container, containerIp, gateway, port, host, providerPort, policyId, pid: child.pid, startTicks, audit };
  try {
    syncFile(location(container), JSON.stringify(state));
    installPolicy(state);
    await verifyState(state);
  } catch (error) {
    try {
      const status = JSON.parse(docker("inspect", container))[0]?.State;
      if (status?.Running && !status.Paused) docker("pause", container);
    } catch (pauseError) {
      throw new AggregateError([error, pauseError], "Egress verification failed; worker could not be paused, preserving firewall rules");
    }
    try { removePolicy(state); } catch (cleanupError) { throw new AggregateError([error, cleanupError], "Egress preparation failed after worker pause and cleanup failed"); }
    throw error;
  }
  console.log(JSON.stringify({ proxy: `http://${gateway}:${port}`, policyId, container, providerHost: host, providerPort }));
  } finally {
    rmSync(reservation, { recursive: true, force: true });
  }
}
function ruleOption(words, option) {
  const index = words.indexOf(option);
  return index < 0 ? undefined : words[index + 1];
}

function chainCanAccept(chain, visiting = new Set(), cache = new Map(), knownChains) {
  if (!knownChains.has(chain)) return true;
  if (cache.has(chain)) return cache.get(chain);
  if (visiting.has(chain)) return true;
  visiting.add(chain);
  const lines = firewall("-S", chain).split("\n").filter((line) => line.startsWith("-A "));
  for (const line of lines) {
    const { kind, target } = ruleTarget(line);
    if (target === "ACCEPT" || kind === "-g"
      || (!["DROP", "REJECT", "RETURN"].includes(target)
        && chainCanAccept(target, visiting, cache, knownChains))) {
      visiting.delete(chain);
      cache.set(chain, true);
      return true;
    }
  }
  visiting.delete(chain);
  cache.set(chain, false);
  return false;
}

function verifyChainOrder(chain, state) {
  const lines = firewall("-S", chain).split("\n").filter((line) => line.startsWith("-A "));
  const rules = lines.map((line) => {
    const words = shellWords(line);
    return { line, words, ...ruleTarget(line) };
  });
  const positions = rules.flatMap((rule, index) => ruleOption(rule.words, "--comment") === state.policyId ? [index] : []);
  if (positions.length !== 2 || positions[1] !== positions[0] + 1
    || rules[positions[0]].kind !== "-j" || rules[positions[0]].target !== "ACCEPT"
    || rules[positions[1]].kind !== "-j" || rules[positions[1]].target !== "REJECT") {
    throw new Error(`${chain} scoped accept/reject rules are missing or out of order`);
  }
  const knownChains = new Set(firewall("-S").split("\n").flatMap((line) => {
    const match = line.match(/^-(?:N|P) (\S+)/);
    return match ? [match[1]] : [];
  }));
  const cache = new Map();
  for (const rule of rules.slice(0, positions[0])) {
    const source = ruleOption(rule.words, "-s");
    const hasNegatedSource = rule.words.some((word, index) => word === "!" && rule.words[index + 1] === "-s");
    const comment = ruleOption(rule.words, "--comment");
    if (rule.kind === "-j" && ["ACCEPT", "RETURN"].includes(rule.target)) {
      if (!source || !/^(?:\d{1,3}\.){3}\d{1,3}(?:\/32)?$/.test(source)
        || hasNegatedSource || source.split("/")[0] === state.containerIp
        || !/^capstan-m1-[0-9a-f]{24}$/.test(comment ?? "")) {
        throw new Error(`${chain} has a preceding rule that can bypass the scoped policy`);
      }
      continue;
    }
    if (rule.kind === "-g" || (!["DROP", "REJECT"].includes(rule.target)
      && chainCanAccept(rule.target, new Set(), cache, knownChains))) {
      throw new Error(`${chain} has a preceding jump/goto chain that can accept or bypass traffic before the scoped policy`);
    }
  }
}

export function verifyForwardRouteRules(forwardRules) {
  const rules = forwardRules.split("\n").filter((line) => line.startsWith("-A ")).map((line) => {
    const words = shellWords(line);
    return { words, ...ruleTarget(line) };
  });
  const userIndex = rules.findIndex((rule) => rule.kind === "-j" && rule.target === "DOCKER-USER"
    && rule.words.length === 4 && rule.words[0] === "-A" && rule.words[1] === "FORWARD");
  if (userIndex < 0) throw new Error("FORWARD does not route worker traffic through DOCKER-USER");
  for (const rule of rules.slice(0, userIndex)) {
    if (rule.kind === "-g" || !["DROP", "REJECT"].includes(rule.target)) {
      throw new Error("FORWARD has a preceding rule that can bypass DOCKER-USER for worker traffic");
    }
  }
}
function verifyForwardRoute(state) {
  verifyForwardRouteRules(firewall("-S", "FORWARD"));
}

async function verifyState(state) {
  if (!hasRule(acceptArgs(state)) || !hasRule(rejectArgs(state))
    || !hasRule(inputAcceptArgs(state)) || !hasRule(inputRejectArgs(state))) {
    throw new Error("Egress firewall rules are absent");
  }
  verifyChainOrder("DOCKER-USER", state);
  verifyForwardRoute(state);
  verifyChainOrder("INPUT", state);
  proxyIdentity(state.pid, state.startTicks, "--check");
  let providerConnections = 0;
  const proxyEvents = [];
  if (existsSync(state.audit)) {
    const audit = readFileSync(state.audit, "utf8");
    if (!audit.endsWith("\n")) throw new Error("Truncated provider connection audit");
    for (const line of audit.slice(0, -1).split("\n")) {
      const entry = JSON.parse(line);
      if (entry.event === "provider_connect") {
        if (entry.host !== state.host || !sniMatchesHost(entry.sni, state.host) || entry.port !== state.providerPort || !publicIPv4(entry.address)) {
          throw new Error("Provider connection audit does not match policy");
        }
        providerConnections += 1;
      } else if (!["http_denied", "connect_attempt", "connect_denied", "tls_denied", "upstream_failed"].includes(entry.event)) {
        throw new Error("Unknown provider proxy audit event");
      }
      proxyEvents.push(entry);
    }
  }
  return { container: state.container, policyId: state.policyId, proxy: `http://${state.gateway}:${state.port}`,
    providerHost: state.host, providerPort: state.providerPort, providerConnections, proxyEvents,
    firewall: { forwarding: "DOCKER-USER accept-proxy-then-reject", hostInput: "INPUT accept-proxy-then-reject" } };
}
async function main() {
  const action = process.argv[2];
  if (action === "serve") { await serve(); return; }
  if (action === "prepare") { await prepare(); return; }
  const container = validatedContainer();
  if (action === "cleanup-container") {
    if (existsSync(`${location(container)}.prepare`))
      throw new Error("Egress preparation still owns this container; cleanup cannot race policy installation");
    if (!existsSync(location(container))) { console.log(JSON.stringify({ removed: true, alreadyAbsent: true })); return; }
    const stored = JSON.parse(readFileSync(location(container), "utf8"));
    if (stored.container !== container || !/^capstan-m1-[0-9a-f]{24}$/.test(stored.policyId))
      throw new Error("Invalid persisted egress policy identity");
    removePolicy(stored);
    console.log(JSON.stringify({ removed: true, policyId: stored.policyId }));
    return;
  }
  const policyId = option("policy-id");
  if (!/^capstan-m1-[0-9a-f]{24}$/.test(policyId)) throw new Error("Invalid policy ID");
  if (action === "cleanup" && existsSync(`${location(container)}.prepare`))
    throw new Error("Egress preparation still owns this container; cleanup cannot race policy installation");
  if (action === "cleanup" && !existsSync(location(container))) { console.log(JSON.stringify({ removed: true, alreadyAbsent: true })); return; }
  const state = readState(container, policyId);
  if (action === "verify") { console.log(JSON.stringify(await verifyState(state))); return; }
  if (action === "probe-deny") {
    await verifyState(state);
    if (!JSON.parse(docker("inspect", container))[0]?.State?.Running) throw new Error("Container is not running");
    const attempt = spawnSync("docker", ["exec", container, "/usr/bin/timeout", "3", "/bin/bash", "-c", "exec 3<>/dev/tcp/1.1.1.1/443"],
      { encoding: "utf8", timeout: 7000 });
    if (attempt.status === null || attempt.status === 0 || attempt.status === 124 || !/connect:|refused|unreachable|denied/i.test(attempt.stderr)) {
      throw new Error(`Direct outbound probe inconclusive (exit ${attempt.status}): ${attempt.stderr.trim()}`);
    }
    const listener = net.createServer((socket) => socket.destroy());
    await new Promise((resolve, reject) => {
      listener.once("error", reject);
      listener.listen(0, state.gateway, resolve);
    });
    let hostAttempt;
    const hostPort = listener.address().port;
    try {
      hostAttempt = spawnSync("docker", ["exec", container, "/usr/bin/timeout", "3", "/bin/bash", "-c", `exec 3<>/dev/tcp/${state.gateway}/${hostPort}`],
        { encoding: "utf8", timeout: 7000 });
    } finally {
      await new Promise((resolve) => listener.close(resolve));
    }
    if (hostAttempt.status === null || hostAttempt.status === 0 || hostAttempt.status === 124
      || !/connect:|refused|unreachable|denied/i.test(hostAttempt.stderr)) {
      throw new Error(`Host INPUT probe inconclusive (exit ${hostAttempt.status}): ${hostAttempt.stderr.trim()}`);
    }
    console.log(JSON.stringify({ denied: true, hostDenied: true, policyId, attempted: "1.1.1.1:443", hostAttempted: `${state.gateway}:${hostPort}`, exitCode: attempt.status, hostExitCode: hostAttempt.status }));
    return;
  }
  if (action === "cleanup") { removePolicy(state); console.log(JSON.stringify({ removed: true, policyId })); return; }
  throw new Error(`Unknown egress action ${action}`);
}
if (process.argv[1] && path.resolve(process.argv[1]) === SELF) {
  main().catch((error) => { console.error(error instanceof Error ? error.message : String(error)); process.exitCode = 1; });
}

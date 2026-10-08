// Measurements against a scratch daemon (never the live one).
//   node bench.mjs latency <command> <calls>        sequential calls; prints p50/p95/max in ms
//   node bench.mjs replay <rate/s> <seconds>        status at a fixed rate (open loop); prints p50/p95
//   node bench.mjs cpu <pid> <seconds> <runs>       /proc utime+stime of the daemon, % of one core per run
//   node bench.mjs mem <pid>                        VmRSS, RssAnon, RssFile
// Env: SCRATCH (default /tmp/capstan-prof), DIST (default $SCRATCH/dist-base)
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const scratch = process.env.SCRATCH ?? "/tmp/capstan-prof";
const dist = process.env.DIST ?? path.join(scratch, "dist-base");
const { callDaemon } = await import(pathToFileURL(path.join(dist, "src/client.js")).href);
const socket = path.join(scratch, "root/.capstan/state/control.sock");
const key = fs.readFileSync(path.join(scratch, "root/.capstan/operator.key"), "utf8").trim();
if (!socket.startsWith("/tmp/")) throw new Error("scratch only");

const pct = (xs, p) => xs.slice().sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor((p / 100) * xs.length))];
const summary = (xs) => `n=${xs.length} p50=${pct(xs, 50).toFixed(1)}ms p95=${pct(xs, 95).toFixed(1)}ms max=${Math.max(...xs).toFixed(1)}ms`;
async function one(command, args = []) {
  const t = performance.now();
  const r = await callDaemon(socket, key, command, args);
  if (!r.response.ok) throw new Error(`${command}: ${JSON.stringify(r.response)}`);
  return performance.now() - t;
}
const [mode, ...rest] = process.argv.slice(2);
if (mode === "latency") {
  const [command, calls] = rest;
  for (let i = 0; i < 5; i++) await one(command);
  const xs = [];
  for (let i = 0; i < Number(calls); i++) xs.push(await one(command));
  console.log(`${command} ${summary(xs)}`);
} else if (mode === "replay") {
  const [rate, seconds] = rest.map(Number);
  const xs = [];
  const pending = [];
  let failed = 0;
  const end = Date.now() + seconds * 1000;
  for (let n = 0; Date.now() < end; n++) {
    pending.push(one("status").then((ms) => xs.push(ms), () => { failed++; }));
    await new Promise((r) => setTimeout(r, 1000 / rate));
  }
  await Promise.all(pending);
  console.log(`replay ${rate}/s ${seconds}s ${summary(xs)} failed=${failed}`);
} else if (mode === "cpu") {
  // v030: also children (cutime+cstime of reaped children, which is where the herdr spawns land), per-thread CPU,
  // the shim's call count and the 1/5/15-minute load averages at the start and the end of each run.
  const [pid, seconds, runs] = rest.map(Number);
  const ticks = Number(process.env.CLK_TCK ?? 100);
  const calls = () => { try { return fs.readFileSync(path.join(scratch, "herdr-calls.log"), "utf8").split("\n").filter(Boolean).length; } catch { return 0; } };
  const read = () => {
    const f = fs.readFileSync(`/proc/${pid}/stat`, "utf8").split(") ")[1].split(" ");
    const threads = {};
    for (const t of fs.readdirSync(`/proc/${pid}/task`)) {
      try {
        const raw = fs.readFileSync(`/proc/${pid}/task/${t}/stat`, "utf8");
        const g = raw.split(") ")[1].split(" ");
        threads[`${raw.slice(raw.indexOf("(") + 1, raw.lastIndexOf(")"))}:${t}`] = Number(g[11]) + Number(g[12]);
      } catch {}
    }
    return { u: Number(f[11]), s: Number(f[12]), cu: Number(f[13]), cs: Number(f[14]), threads, calls: calls() };
  };
  const load = () => fs.readFileSync("/proc/loadavg", "utf8").split(" ").slice(0, 3).join("/");
  for (let i = 0; i < runs; i++) {
    const a = read(), t = Date.now(), l0 = load();
    await new Promise((r) => setTimeout(r, seconds * 1000));
    const b = read();
    const wall = (Date.now() - t) / 1000;
    const pc = (x) => ((x / ticks / wall) * 100).toFixed(1);
    const own = b.u + b.s - a.u - a.s;
    const kids = b.cu + b.cs - a.cu - a.cs;
    const perThread = Object.keys(b.threads).map((k) => [k, ((b.threads[k] - (a.threads[k] ?? 0)) / ticks / wall) * 100]).filter(([, v]) => v >= 0.05).map(([k, v]) => `${k}=${v.toFixed(1)}%`).join(" ");
    console.log(`cpu run ${i + 1}: own ${pc(own)}% (user ${pc(b.u - a.u)} sys ${pc(b.s - a.s)}) children ${pc(kids)}% (user ${pc(b.cu - a.cu)} sys ${pc(b.cs - a.cs)}) total ${pc(own + kids)}% of one core over ${wall.toFixed(0)}s; herdr calls ${b.calls - a.calls} (${((b.calls - a.calls) / wall).toFixed(2)}/s); threads ${perThread}; load ${l0} -> ${load()}`);
  }
} else if (mode === "mem") {
  const s = fs.readFileSync(`/proc/${Number(rest[0])}/status`, "utf8");
  for (const k of ["VmRSS", "RssAnon", "RssFile"]) console.log(s.match(new RegExp(`${k}:\\s+(\\d+) kB`))?.[0].replace(/\s+/g, " "));
} else throw new Error("unknown mode");

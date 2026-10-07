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
  const [pid, seconds, runs] = rest.map(Number);
  const ticks = Number(process.env.CLK_TCK ?? 100);
  const read = () => {
    const f = fs.readFileSync(`/proc/${pid}/stat`, "utf8").split(") ")[1].split(" ");
    return Number(f[11]) + Number(f[12]);
  };
  for (let i = 0; i < runs; i++) {
    const a = read(), t = Date.now();
    await new Promise((r) => setTimeout(r, seconds * 1000));
    const used = (read() - a) / ticks, wall = (Date.now() - t) / 1000;
    console.log(`cpu run ${i + 1}: ${((used / wall) * 100).toFixed(1)}% of one core over ${wall.toFixed(0)}s`);
  }
} else if (mode === "mem") {
  const s = fs.readFileSync(`/proc/${Number(rest[0])}/status`, "utf8");
  for (const k of ["VmRSS", "RssAnon", "RssFile"]) console.log(s.match(new RegExp(`${k}:\\s+(\\d+) kB`))?.[0].replace(/\s+/g, " "));
} else throw new Error("unknown mode");

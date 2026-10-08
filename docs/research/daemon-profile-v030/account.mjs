// Splits the process CPU of plain runs (no profiler) into named costs: node account.mjs <prof dir> <profile label> <plain label>...
// MainThread CPU (from /proc/<pid>/task, mean over the minute windows of the plain runs) is split by the owner shares of the profiled run
// of the same build (attribute.mjs rules); V8Worker threads are GC helper threads, libuv-worker threads run the async fs reads of the
// /proc scan, and children are the herdr (here: shim) processes whose CPU the kernel adds to cutime/cstime when they are reaped.
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
const [dir, profile, ...plain] = process.argv.slice(2);
const here = path.dirname(new URL(import.meta.url).pathname);
const attribution = execFileSync("node", [path.join(here, "attribute.mjs"), path.join(dir, profile, "cpu.cpuprofile")], { env: { ...process.env, TOP: "200" }, encoding: "utf8" });
const shares = new Map();
for (const line of attribution.split("\n")) {
  const m = line.match(/^\s*([\d.]+)% of busy\s+[\d.]+% of wall\s+\d+ ms\s+(.*)$/);
  if (m) shares.set(m[2], Number(m[1]) / 100);
}
const group = (name) => {
  if (/herdr agent get|^other: spawn |runJson|(anon) src\/herdr\/runner/.test(name)) return "herdr `agent get` spawns (JS side: spawn, pipes, JSON)";
  if (/process-activity/.test(name)) return "process-activity probe (`pane process-info` spawn + /proc scan)";
  if (/judgeWakes/.test(name)) return "driver: PM mail re-read (#judgeWakes, #judgeStale)";
  if (/report relay/.test(name)) return "report relay tick (every 2 s)";
  if (/#advance/.test(name)) return "driver: #advance (advanceMessaging, delivery notices)";
  if (/garbage|program/.test(name)) return "V8 GC and native on the main thread";
  if (/writeHeapSnapshot/.test(name)) return "(profiling artifact, excluded)";
  return "other main-thread work (deliver, supervision, request handling, sqlite helpers, startup)";
};
const grouped = new Map();
let excluded = 0;
for (const [name, s] of shares) {
  const g = group(name);
  if (g.startsWith("(profiling")) { excluded += s; continue; }
  grouped.set(g, (grouped.get(g) ?? 0) + s);
}
const norm = 1 - excluded;
const num = (re, l) => Number(l.match(re)?.[1] ?? 0);
for (const label of plain) {
  const lines = fs.readFileSync(path.join(dir, label, "cpu.txt"), "utf8").split("\n").filter((l) => l.startsWith("cpu run"));
  const mean = (f) => lines.reduce((a, l) => a + f(l), 0) / lines.length;
  const main = mean((l) => num(/MainThread:\d+=([\d.]+)%/, l));
  const v8 = mean((l) => [...l.matchAll(/V8Worker:\d+=([\d.]+)%/g)].reduce((a, m) => a + Number(m[1]), 0));
  const uv = mean((l) => [...l.matchAll(/libuv-worker:\d+=([\d.]+)%/g)].reduce((a, m) => a + Number(m[1]), 0));
  const kids = mean((l) => num(/children ([\d.]+)%/, l));
  const total = mean((l) => num(/total ([\d.]+)%/, l));
  const rows = [...grouped].map(([g, s]) => [g, (main * s) / norm]);
  rows.push(["helper threads: V8 GC workers (V8Worker x4)", v8], ["helper threads: libuv workers (async fs reads of the /proc scan)", uv], ["child processes (shim for `herdr`): cutime + cstime", kids]);
  const sum = rows.reduce((a, [, v]) => a + v, 0);
  console.log(`\n### ${label}: process total ${total.toFixed(2)}% of one core (MainThread ${main.toFixed(2)}%), ${lines.length} one-minute windows\n`);
  console.log("| Cost | % of one core | Share of process CPU |\n| --- | --- | --- |");
  for (const [g, v] of rows.sort((a, b) => b[1] - a[1])) console.log(`| ${g} | ${v.toFixed(2)} | ${((v / total) * 100).toFixed(0)}% |`);
  console.log(`| sum of the named costs | ${sum.toFixed(2)} | ${((sum / total) * 100).toFixed(0)}% |`);
}

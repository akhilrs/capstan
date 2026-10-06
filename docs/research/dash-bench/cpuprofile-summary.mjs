#!/usr/bin/env node
// Summarises a .cpuprofile: self time by function, by file and by area. usage: node cpuprofile-summary.mjs FILE [top]
import fs from "node:fs";
const [file, topArg] = process.argv.slice(2);
const top = Number(topArg ?? 25);
const prof = JSON.parse(fs.readFileSync(file, "utf8"));
const byId = new Map(prof.nodes.map((n) => [n.id, n]));
const self = new Map();
const dt = prof.timeDeltas;
for (let i = 0; i < prof.samples.length; i++) self.set(prof.samples[i], (self.get(prof.samples[i]) ?? 0) + (dt[i] ?? 0));
const total = [...self.values()].reduce((a, b) => a + b, 0);
const idle = [...self.entries()].filter(([id]) => byId.get(id).callFrame.functionName === "(idle)").reduce((a, [, v]) => a + v, 0);
const fn = new Map(), fileMap = new Map();
for (const [id, t] of self) {
  const cf = byId.get(id).callFrame;
  const short = cf.url.replace(/^.*node_modules\//, "nm/").replace(/^file:\/\/.*\/dist\//, "dist/");
  const k = `${cf.functionName || "(anon)"} ${short}:${cf.lineNumber + 1}`;
  fn.set(k, (fn.get(k) ?? 0) + t);
  fileMap.set(short || cf.functionName, (fileMap.get(short || cf.functionName) ?? 0) + t);
}
const ms = (u) => (u / 1000).toFixed(0).padStart(7) + " ms " + ((u / (total - idle)) * 100).toFixed(1).padStart(5) + "%";
console.log(`wall ${(total / 1e6).toFixed(1)} s, idle ${(idle / 1e6).toFixed(1)} s, busy ${((total - idle) / 1e6).toFixed(2)} s`);
console.log("-- top self time by function (share of busy time)");
for (const [k, v] of [...fn].filter(([k]) => !k.startsWith("(idle)")).sort((a, b) => b[1] - a[1]).slice(0, top)) console.log(ms(v), k);
console.log("-- top self time by file");
for (const [k, v] of [...fileMap].filter(([k]) => k !== "(idle)").sort((a, b) => b[1] - a[1]).slice(0, top)) console.log(ms(v), k);

#!/usr/bin/env node
// Inclusive (self + callees) time per function name of a .cpuprofile, counted once per sample stack. usage: node cpuprofile-inclusive.mjs FILE name1,name2,...
import fs from "node:fs";
const [file, names] = process.argv.slice(2);
const want = new Set(names.split(","));
const p = JSON.parse(fs.readFileSync(file, "utf8"));
const by = new Map(p.nodes.map((n) => [n.id, n]));
const parent = new Map();
for (const n of p.nodes) for (const c of n.children ?? []) parent.set(c, n.id);
const incl = new Map();
p.samples.forEach((id, i) => {
  const seen = new Set();
  for (let c = id; c !== undefined; c = parent.get(c)) {
    const f = by.get(c).callFrame.functionName;
    if (want.has(f) && !seen.has(f)) { seen.add(f); incl.set(f, (incl.get(f) ?? 0) + p.timeDeltas[i]); }
  }
});
for (const [k, v] of [...incl].sort((a, b) => b[1] - a[1])) console.log(String((v / 1000) | 0).padStart(7), "ms", k);

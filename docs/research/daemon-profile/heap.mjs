// Self size by node type and constructor from a .heapsnapshot: node heap.mjs <file> [n]
import fs from "node:fs";
const snap = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const n = Number(process.argv[3] ?? 25);
const f = snap.snapshot.meta.node_fields;
const stride = f.length;
const iType = f.indexOf("type"),
  iName = f.indexOf("name"),
  iSize = f.indexOf("self_size");
const types = snap.snapshot.meta.node_types[0];
const strings = snap.strings;
const byType = new Map(),
  byName = new Map();
let total = 0;
for (let i = 0; i < snap.nodes.length; i += stride) {
  const type = types[snap.nodes[i + iType]],
    size = snap.nodes[i + iSize];
  total += size;
  byType.set(type, (byType.get(type) ?? 0) + size);
  if (
    type === "object" ||
    type === "closure" ||
    type === "regexp" ||
    type === "native" ||
    type === "array"
  ) {
    const k = `${type} ${strings[snap.nodes[i + iName]]}`;
    const e = byName.get(k) ?? { size: 0, count: 0 };
    e.size += size;
    e.count++;
    byName.set(k, e);
  }
}
const mb = (x) => (x / 1048576).toFixed(2).padStart(7);
console.log(`live heap ${mb(total)} MB, ${snap.nodes.length / stride} nodes`);
for (const [t, s] of [...byType].sort((a, b) => b[1] - a[1]))
  console.log(`${mb(s)} MB  ${t}`);
console.log("\nby constructor (self size):");
for (const [k, e] of [...byName]
  .sort((a, b) => b[1].size - a[1].size)
  .slice(0, n))
  console.log(`${mb(e.size)} MB  ${String(e.count).padStart(7)}x  ${k}`);

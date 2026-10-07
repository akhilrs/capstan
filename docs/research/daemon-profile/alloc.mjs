// Bytes allocated per function while N status calls run in process (V8 sampling heap profiler, collected objects included).
//   node alloc.mjs <dist dir> [calls=100]
import fs from "node:fs";
import inspector from "node:inspector/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "..",
);
const fixtures = path.join(root, "test", "fixtures", "daemon-cost");
const { buildLedger } = await import(path.join(fixtures, "ledger.mjs"));
const { openFor, serve } = await import(path.join(fixtures, "capture.mjs"));
const dist = process.argv[2];
const calls = Number(process.argv[3] ?? 100);
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "capstan-alloc-"));
const built = await buildLedger(dist, dir);
const core = await openFor(dist, built);
const served = await serve(dist, core, built);
for (let i = 0; i < 20; i++) await served.call("status");
const session = new inspector.Session();
session.connect();
await session.post("HeapProfiler.startSampling", {
  samplingInterval: 2048,
  includeObjectsCollectedByMajorGC: true,
  includeObjectsCollectedByMinorGC: true,
});
for (let i = 0; i < calls; i++) await served.call("status");
const { profile } = await session.post("HeapProfiler.stopSampling");
const bySelf = new Map();
let total = 0;
const walk = (node) => {
  const f = node.callFrame;
  const key = `${f.functionName || "(anon)"} ${f.url.replace(/^.*\/(dist[^/]*|src)\//, "")}:${f.lineNumber + 1}`;
  bySelf.set(key, (bySelf.get(key) ?? 0) + node.selfSize);
  total += node.selfSize;
  node.children.forEach(walk);
};
walk(profile.head);
console.log(
  `${(total / calls / 1024).toFixed(0)} KB allocated per status call (${calls} calls, sampled)`,
);
for (const [k, v] of [...bySelf].sort((a, b) => b[1] - a[1]).slice(0, 14))
  console.log(
    `${((v / total) * 100).toFixed(1).padStart(5)}%  ${(v / calls / 1024).toFixed(0).padStart(6)} KB  ${k}`,
  );
await served.close();
core.close();
fs.rmSync(dir, { recursive: true, force: true });
process.exit(0);

// Calls the daemon `status` command N times like the dash poller does; prints response bytes, top-level key sizes, wall latency and the dash model's counts.
// usage: node status-probe.mjs PROJECT_DIR [N]   (run with CAPSTAN_SOCKET/CAPSTAN_TOKEN unset)
import fs from "node:fs";
import path from "node:path";
const W = new URL("../../../dist/src/", import.meta.url);
const { callDaemon } = await import(new URL("client.js", W));
const { buildDashModel } = await import(new URL("dash/model.js", W));
const dir = path.resolve(process.argv[2]);
const n = Number(process.argv[3] ?? 20);
const state = path.join(dir, ".capstan/state");
const cred = fs.readFileSync(path.join(dir, ".capstan/operator.key"), "utf8").trim();
const sock = path.join(state, "control.sock");
const lat = []; let result;
for (let i = 0; i < n; i++) {
  const t = process.hrtime.bigint();
  const r = await callDaemon(sock, cred, "status");
  lat.push(Number(process.hrtime.bigint() - t) / 1e6);
  result = r.response.result;
}
lat.sort((a, b) => a - b);
const bytes = Buffer.byteLength(JSON.stringify(result));
console.log(JSON.stringify({ n, bytes, p50_ms: +lat[n >> 1].toFixed(1), min_ms: +lat[0].toFixed(1), max_ms: +lat[n - 1].toFixed(1) }));
const sizes = Object.entries(result).map(([k, v]) => [k, Buffer.byteLength(JSON.stringify(v))]).sort((a, b) => b[1] - a[1]);
console.log("key bytes:", sizes.slice(0, 8).map(([k, v]) => `${k}=${v}`).join(" "));
const reps = 200; const t1 = process.hrtime.bigint();
for (let i = 0; i < reps; i++) JSON.parse(JSON.stringify(result));
console.log("stringify+parse ms each:", (Number(process.hrtime.bigint() - t1) / 1e6 / reps).toFixed(2));
const t2 = process.hrtime.bigint(); let model;
for (let i = 0; i < reps; i++) model = buildDashModel(result, Date.now(), 3);
console.log("buildDashModel ms each:", (Number(process.hrtime.bigint() - t2) / 1e6 / reps).toFixed(2), "counts:", JSON.stringify(model.counts));

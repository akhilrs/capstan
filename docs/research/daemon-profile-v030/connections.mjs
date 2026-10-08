// Per-connection memory of the scratch daemon: node connections.mjs <pid> <connections>
// Opens N idle unix-socket connections (no request sent), reads RssAnon, then sends one `status` on each in parallel and
// reads RssAnon at the end of the burst and 5 s after the connections closed. Scratch daemon only.
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { pathToFileURL } from "node:url";

const scratch = process.env.SCRATCH ?? "/tmp/capstan-prof-v030/s";
const dist = process.env.DIST ?? path.join(scratch, "dist-base");
const [pid, count] = process.argv.slice(2).map(Number);
const socket = path.join(scratch, "root/.capstan/state/control.sock");
if (!socket.startsWith("/tmp/")) throw new Error("scratch only");
const key = fs.readFileSync(path.join(scratch, "root/.capstan/operator.key"), "utf8").trim();
const { callDaemon } = await import(pathToFileURL(path.join(dist, "src/client.js")).href);
const anon = () => Number(fs.readFileSync(`/proc/${pid}/status`, "utf8").match(/RssAnon:\s+(\d+) kB/)[1]);
const idle = () => new Promise((r) => setTimeout(r, 3000));
await idle();
const before = anon();
const sockets = await Promise.all(Array.from({ length: count }, () => new Promise((resolve, reject) => {
  const s = net.createConnection(socket, () => resolve(s));
  s.on("error", reject);
})));
await idle();
const held = anon();
for (const s of sockets) s.destroy();
const t = performance.now();
const results = await Promise.all(Array.from({ length: count }, () => callDaemon(socket, key, "status", [])));
const burstMs = performance.now() - t;
const burst = anon();
await new Promise((r) => setTimeout(r, 5000));
const after = anon();
console.log(JSON.stringify({ connections: count, rssAnonKbBefore: before, idleConnectionsHeldKb: held, perIdleConnectionKb: +((held - before) / count).toFixed(1), afterStatusBurstKb: burst, burstKbPerConnection: +((burst - before) / count).toFixed(1), burstMs: +burstMs.toFixed(0), failed: results.filter((r) => !r.response.ok).length, fiveSecondsAfterKb: after }));

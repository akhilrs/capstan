// Preload (node --import) that counts process.stdout.write calls, bytes and time spent inside them; writes totals to $COUNT_WRITES_OUT every 5 s.
import fs from "node:fs";
const out = process.env.COUNT_WRITES_OUT ?? "/tmp/count-writes.json";
const orig = process.stdout.write.bind(process.stdout);
let calls = 0, bytes = 0, ns = 0n, big = 0;
const started = Date.now();
process.stdout.write = (chunk, ...rest) => {
  const t = process.hrtime.bigint();
  const r = orig(chunk, ...rest);
  ns += process.hrtime.bigint() - t;
  calls++; const n = Buffer.byteLength(chunk); bytes += n; if (n > 4000) big++;
  return r;
};
setInterval(() => fs.writeFileSync(out, JSON.stringify({ seconds: (Date.now() - started) / 1000, calls, bytes, writeMs: Number(ns / 1000000n), writesOver4kB: big })), 5000).unref();

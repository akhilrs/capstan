// Preload (node --import): every 5 s appends {t, perfEntries, heapMB, rssMB} to $PERF_OUT, to test whether React's dev-mode performance.measure entries pile up.
import fs from "node:fs";
const out = process.env.PERF_OUT ?? "/tmp/perf-entries.jsonl";
const t0 = Date.now();
setInterval(() => {
  const m = process.memoryUsage();
  fs.appendFileSync(out, JSON.stringify({ t: Math.round((Date.now() - t0) / 1000), perfEntries: performance.getEntries().length, heapMB: Math.round(m.heapUsed / 1048576), rssMB: Math.round(m.rss / 1048576) }) + "\n");
}, 5000).unref();

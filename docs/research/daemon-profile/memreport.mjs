// Preloaded with `node --import memreport.mjs`: on SIGURG writes $MEMREPORT_OUT with what the process holds,
// so RssAnon can be split into V8 heap spaces, external memory and the rest (malloc arenas, SQLite page caches).
import fs from "node:fs";
import v8 from "node:v8";

process.on("SIGURG", () => {
  const m = process.memoryUsage();
  const smaps = fs.readFileSync("/proc/self/smaps", "utf8").split("\n");
  const maps = [];
  let current;
  for (const line of smaps) {
    if (/^[0-9a-f]+-[0-9a-f]+ /.test(line)) {
      current = {
        range: line.split(" ")[0],
        name: line.split(/\s+/)[5] ?? "",
        anonKb: 0,
        rssKb: 0,
      };
      maps.push(current);
    } else if (current && line.startsWith("Anonymous:"))
      current.anonKb = Number(line.match(/(\d+)/)[1]);
    else if (current && line.startsWith("Rss:"))
      current.rssKb = Number(line.match(/(\d+)/)[1]);
  }
  const anon = maps.filter((x) => x.anonKb > 0);
  const total = anon.reduce((a, x) => a + x.anonKb, 0);
  const report = {
    memoryUsageMb: Object.fromEntries(
      Object.entries(m).map(([k, v]) => [k, +(v / 1048576).toFixed(1)]),
    ),
    v8Spaces: v8.getHeapSpaceStatistics().map((s) => ({
      space: s.space_name,
      sizeMb: +(s.space_size / 1048576).toFixed(1),
      usedMb: +(s.space_used_size / 1048576).toFixed(1),
      availableMb: +(s.space_available_size / 1048576).toFixed(1),
    })),
    heap: Object.fromEntries(
      Object.entries(v8.getHeapStatistics()).map(([k, v]) => [
        k,
        typeof v === "number" ? +(v / 1048576).toFixed(1) : v,
      ]),
    ),
    anonTotalMb: +(total / 1024).toFixed(1),
    largestAnonMappingsMb: anon
      .sort((a, b) => b.anonKb - a.anonKb)
      .slice(0, 8)
      .map((x) => ({
        range: x.range,
        name: x.name,
        anonMb: +(x.anonKb / 1024).toFixed(1),
      })),
    smallAnonMappings: anon.filter((x) => x.anonKb < 1024).length,
  };
  fs.writeFileSync(
    process.env.MEMREPORT_OUT ?? "/tmp/memreport.json",
    `${JSON.stringify(report, null, 1)}\n`,
  );
});

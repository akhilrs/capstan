// Summarises run.sh outputs: node summarize.mjs <prof dir> <label>...   prints one line per run (mean over the minute windows)
import fs from "node:fs";
import path from "node:path";
const [dir, ...labels] = process.argv.slice(2);
const num = (re, text) => Number(text.match(re)?.[1]);
for (const label of labels) {
  const base = path.join(dir, label);
  const cpu = fs.readFileSync(path.join(base, "cpu.txt"), "utf8");
  const runs = cpu.split("\n").filter((l) => l.startsWith("cpu run"));
  const field = (re) => runs.map((l) => num(re, l));
  const mean = (xs) => xs.reduce((a, b) => a + b, 0) / xs.length;
  const own = field(/own ([\d.]+)%/), kids = field(/children ([\d.]+)%/), total = field(/total ([\d.]+)%/), calls = field(/herdr calls (\d+)/);
  const loads = runs.map((l) => l.match(/load ([\d./]+) ->/)[1]);
  const mem = fs.readFileSync(path.join(base, "mem.txt"), "utf8");
  const rep = JSON.parse(fs.readFileSync(path.join(base, "memreport.json"), "utf8"));
  const space = (n) => rep.v8Spaces.find((s) => s.space === n)?.sizeMb ?? 0;
  const used = (n) => rep.v8Spaces.find((s) => s.space === n)?.usedMb ?? 0;
  const heapMapping = rep.largestAnonMappingsMb.find((m) => m.name === "[heap]")?.anonMb ?? 0;
  console.log(JSON.stringify({
    label, minutes: runs.length,
    cpuTotalPct: +mean(total).toFixed(2), cpuOwnPct: +mean(own).toFixed(2), cpuChildrenPct: +mean(kids).toFixed(2),
    perMinuteTotal: total, herdrCallsPerMin: +mean(calls).toFixed(0),
    rssAnonMb: +(num(/RssAnon:\s+(\d+)/, mem) / 1024).toFixed(1), rssFileMb: +(num(/RssFile:\s+(\d+)/, mem) / 1024).toFixed(1),
    youngMbCommitted: space("new_space"), youngUsedMb: used("new_space"), oldMbCommitted: space("old_space"), oldUsedMb: used("old_space"),
    codeMb: space("code_space") + space("trusted_space"), largeObjectMb: space("large_object_space") + space("new_large_object_space"),
    anonTotalMb: rep.anonTotalMb, mallocHeapMappingMb: heapMapping, externalMb: rep.memoryUsageMb.external,
    loadStartOfEachMinute: loads[0] + " ... " + loads.at(-1),
  }));
}

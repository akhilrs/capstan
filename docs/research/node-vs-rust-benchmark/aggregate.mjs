// node aggregate.mjs <results.tsv>: per metric and side, n, median, p95, min, max, and each round's median (the variance).
import { readFileSync } from "node:fs";

const rows = readFileSync(process.argv[2], "utf8")
  .trim()
  .split("\n")
  .map((line) => line.split("\t"));
const by = new Map();
for (const [round, side, metric, value] of rows) {
  const v = Number(value);
  if (!Number.isFinite(v)) continue;
  const key = `${metric}\t${side}`;
  if (!by.has(key)) by.set(key, { all: [], rounds: new Map() });
  const entry = by.get(key);
  entry.all.push(v);
  if (!entry.rounds.has(round)) entry.rounds.set(round, []);
  entry.rounds.get(round).push(v);
}
const sorted = (a) => [...a].sort((x, y) => x - y);
const quantile = (a, p) => {
  const s = sorted(a);
  return s[Math.min(s.length - 1, Math.ceil(p * s.length) - 1)];
};
const median = (a) => {
  const s = sorted(a);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const fmt = (x) => (Math.abs(x) >= 100 ? x.toFixed(0) : x.toFixed(2));
console.log(["metric", "side", "n", "median", "p95", "min", "max", "round_medians"].join("\t"));
for (const [key, entry] of [...by].sort()) {
  const [metric, side] = key.split("\t");
  console.log(
    [
      metric,
      side,
      entry.all.length,
      fmt(median(entry.all)),
      fmt(quantile(entry.all, 0.95)),
      fmt(Math.min(...entry.all)),
      fmt(Math.max(...entry.all)),
      [...entry.rounds.values()].map((r) => fmt(median(r))).join(" / "),
    ].join("\t"),
  );
}

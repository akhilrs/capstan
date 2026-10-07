// Self time per function (and per top-level caller group) from a .cpuprofile: node top.mjs <file> [n]
import fs from "node:fs";
const p = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const n = Number(process.argv[3] ?? 30);
const byId = new Map(p.nodes.map((x) => [x.id, x]));
const self = new Map();
for (let i = 0; i < p.samples.length; i++)
  self.set(
    p.samples[i],
    (self.get(p.samples[i]) ?? 0) + (p.timeDeltas[i] ?? 0),
  );
const total = [...self.values()].reduce((a, b) => a + b, 0);
const fn = new Map();
for (const [id, t] of self) {
  const c = byId.get(id).callFrame;
  const k = `${c.functionName || "(anon)"} ${c.url.replace(/^.*\/dist[^/]*\//, "").replace("node:", "node:")}:${c.lineNumber + 1}`;
  fn.set(k, (fn.get(k) ?? 0) + t);
}
console.log(`total sampled ${(total / 1e6).toFixed(1)}s`);
for (const [k, t] of [...fn].sort((a, b) => b[1] - a[1]).slice(0, n))
  console.log(
    `${((t / total) * 100).toFixed(1).padStart(5)}%  ${(t / 1e3).toFixed(0).padStart(7)}ms  ${k}`,
  );
// inclusive time by first capstan frame below the root, for attribution
const parent = new Map();
for (const x of p.nodes) for (const c of x.children ?? []) parent.set(c, x.id);
const incl = new Map();
for (const [id, t] of self) {
  const seen = new Set();
  for (let cur = id; cur !== undefined; cur = parent.get(cur)) {
    const c = byId.get(cur).callFrame;
    if (!/\/dist[^/]*\//.test(c.url)) continue;
    const k = `${c.functionName || "(anon)"} ${c.url.replace(/^.*\/dist[^/]*\//, "")}:${c.lineNumber + 1}`;
    if (seen.has(k)) continue;
    seen.add(k);
    incl.set(k, (incl.get(k) ?? 0) + t);
  }
}
console.log("\ninclusive (capstan frames):");
for (const [k, t] of [...incl].sort((a, b) => b[1] - a[1]).slice(0, n))
  console.log(`${((t / total) * 100).toFixed(1).padStart(5)}%  ${k}`);

// Busy time (everything but "(idle)") by the loop or command that owns the stack, and by what the self time is spent in.
const OWNERS = [
  ["status command", /\bstatus src\/commands\/status\.js/],
  ["other commands (daemon handle)", /\bhandle src\/daemon\.js/],
  ["report relay tick", /\btick src\/reports\.js/],
  ["driver tick", /#tick src\/driver\.js/],
  ["supervision tick", /src\/supervision\.js/],
  ["notifier", /src\/notifier\.js/],
  [
    "startup recovery (adopt, release, integrations, reviews)",
    /(releaseResources|recoverIntegrations|recoverReviews|adoptAll|closeStaleWaits)/,
  ],
];
const owner = new Map();
let busy = 0;
for (const [id, t] of self) {
  const root = byId.get(id).callFrame;
  if (root.functionName === "(idle)") continue;
  busy += t;
  const names = [];
  for (let cur = id; cur !== undefined; cur = parent.get(cur)) {
    const c = byId.get(cur).callFrame;
    names.push(
      `${c.functionName || "(anon)"} ${c.url.replace(/^.*\/dist[^/]*\//, "")}`,
    );
  }
  const stack = names.join("\n");
  const hit = OWNERS.find(([, re]) => re.test(stack));
  const k = hit
    ? hit[0]
    : `other: ${root.functionName || "(anon)"} ${root.url.replace(/^.*\/dist[^/]*\//, "")}`;
  owner.set(k, (owner.get(k) ?? 0) + t);
}
console.log(
  `\nbusy ${(busy / 1e6).toFixed(1)}s of ${(total / 1e6).toFixed(1)}s sampled (${((busy / total) * 100).toFixed(1)}%); share of busy self time by owner:`,
);
for (const [k, t] of [...owner].sort((a, b) => b[1] - a[1]).slice(0, 12))
  console.log(
    `${((t / busy) * 100).toFixed(1).padStart(5)}%  ${((t / total) * 100).toFixed(1).padStart(5)}% of wall  ${k}`,
  );

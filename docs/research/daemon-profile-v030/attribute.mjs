// Attributes the main-thread self time of a .cpuprofile to named loops and functions: node attribute.mjs <file.cpuprofile> [procCpuMs]
// A sample is owned by the nearest ancestor frame in OWNERS; frames with no capstan ancestor (async fs callbacks) are owned by
// the function name table ASYNC. Output: ms, share of busy (non-idle) samples, share of wall.
import fs from "node:fs";
const p = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const byId = new Map(p.nodes.map((x) => [x.id, x]));
const parent = new Map();
for (const x of p.nodes) for (const c of x.children ?? []) parent.set(c, x.id);
const self = new Map();
for (let i = 0; i < p.samples.length; i++) self.set(p.samples[i], (self.get(p.samples[i]) ?? 0) + (p.timeDeltas[i] ?? 0));
const wall = (p.endTime - p.startTime) / 1000;
const name = (id) => { const c = byId.get(id).callFrame; return `${c.functionName || "(anon)"} ${c.url.replace(/^.*\/dist[^/]*\//, "")}:${c.lineNumber + 1}`; };
// Ordered: the first match walking from the leaf towards the root wins.
const OWNERS = [
  ["driver: herdr agent get per agent (spawn + pipes + JSON)", /#observe |#stateFor|agentObservation/],
  ["driver: process-activity probe (pane process-info + /proc scan)", /#sampleProcesses|HerdrProcessProbe|sample src\/herdr\/process-activity|readProcTable|parseProcStat/],
  ["driver: #judgeWakes / #judgeStale PM message reads", /#judgeWakes|#judgeStale|#pmMessages/],
  ["driver: #advance (advanceMessaging, notices)", /#advance|advanceMessaging|queueMissingDeliveryNotices/],
  ["driver: #deliver / #updateStuck / forget", /#deliver|#updateStuck|#forget|guardedSend/],
  ["driver: other tick work", /#tick src\/driver|tick src\/driver|loop src\/driver/],
  ["report relay tick", /tick src\/reports/],
  ["supervision tick", /src\/supervision/],
  ["restart/operator result poll", /ingestResults|src\/restart/],
  ["daemon request handling (client calls)", /src\/daemon\.js|src\/commands/],
];
const ASYNC = /^(read|fstat|openFileHandle|FastBuffer|close|readFileHandle|open|readFile|decode|lstat|RegExp: \\s\+|readdir|SafePromise|\(anon\) node:internal\/fs)/;
const rows = new Map();
let busy = 0, idle = 0;
for (const [id, t] of self) {
  const leaf = name(id);
  if (leaf.startsWith("(idle)")) { idle += t; continue; }
  busy += t;
  let owner;
  for (let cur = id; cur !== undefined && owner === undefined; cur = parent.get(cur)) {
    const n = name(cur);
    for (const [label, re] of OWNERS) if (re.test(n)) { owner = label; break; }
  }
  if (owner === undefined) owner = ASYNC.test(leaf) ? "driver: process-activity probe (pane process-info + /proc scan)" : leaf.startsWith("(garbage collector)") ? "V8 garbage collector (main thread)" : leaf.startsWith("(program)") ? "(program) native, unattributed by V8" : `other: ${leaf}`;
  rows.set(owner, (rows.get(owner) ?? 0) + t);
}
console.log(`profile wall ${(wall / 1000).toFixed(1)}s, busy ${(busy / 1e6).toFixed(1)}s (${((busy / 1e3 / wall) * 100).toFixed(1)}% of wall)`);
for (const [k, t] of [...rows].sort((a, b) => b[1] - a[1]).slice(0, Number(process.env.TOP ?? 14)))
  console.log(`${((t / busy) * 100).toFixed(1).padStart(5)}% of busy  ${((t / 1e3 / wall) * 100).toFixed(2).padStart(5)}% of wall  ${(t / 1e3).toFixed(0).padStart(7)} ms  ${k}`);

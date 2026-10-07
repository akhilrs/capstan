// Statement counts and SQL time per operation, in-process, on a COPY of a ledger (never the live one).
//   node stmts.mjs <dist dir> <scratch root with .capstan/{operator.key,project.json,state}> [top=15]
// Starts the daemon commands in this process (no driver, no Herdr), patches Statement.get/all/run to count
// and time by SQL text, then measures: the status command, and one pass of each background loop body.
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [dist, root, topArg] = process.argv.slice(2);
const top = Number(topArg ?? 15);
const load = (p) => import(pathToFileURL(path.join(dist, p)).href);
const sqlite = await load("src/controller/sqlite.js");
const { ControllerCore } = await load("src/controller/core.js");
const { startDaemonServer } = await load("src/daemon.js");
const { callDaemon } = await load("src/client.js");

const counts = new Map();
let counting = false;
const sqlOf = new WeakMap();
const proto = sqlite.Statement.prototype;
for (const m of ["get", "all", "run"]) {
  const orig = proto[m];
  proto[m] = function (...a) {
    if (!counting) return orig.apply(this, a);
    const t = performance.now();
    try {
      return orig.apply(this, a);
    } finally {
      const k = (sqlOf.get(this) ?? "?").replace(/\s+/g, " ").trim();
      const e = counts.get(k) ?? { n: 0, ms: 0 };
      e.n++;
      e.ms += performance.now() - t;
      counts.set(k, e);
    }
  };
}
const prep = sqlite.Database.prototype.prepare;
sqlite.Database.prototype.prepare = function (sql) {
  const s = prep.call(this, sql);
  sqlOf.set(s, sql);
  return s;
};

const state = path.join(root, ".capstan/state");
const key = fs
  .readFileSync(path.join(root, ".capstan/operator.key"), "utf8")
  .trim();
const meta = JSON.parse(
  fs.readFileSync(path.join(root, ".capstan/project.json"), "utf8"),
);
const core = await ControllerCore.open({
  stateDirectory: state,
  project: {
    projectId: meta.projectId,
    name: meta.name,
    ownerCredential: key,
    initialInputs: [],
  },
  workspaceRoot: root,
});
const report = (label, fn) => {
  counts.clear();
  counting = true;
  const t = performance.now();
  const out = fn();
  if (!(out instanceof Promise)) counting = false;
  const finish = (ms) => {
    const n = [...counts.values()].reduce((a, e) => a + e.n, 0);
    const sqlMs = [...counts.values()].reduce((a, e) => a + e.ms, 0);
    console.log(
      `\n## ${label}: ${ms.toFixed(1)} ms wall, ${n} statements, ${sqlMs.toFixed(1)} ms in SQL`,
    );
    for (const [k, e] of [...counts]
      .sort((a, b) => b[1].ms - a[1].ms)
      .slice(0, top))
      console.log(
        `${e.ms.toFixed(1).padStart(8)} ms ${String(e.n).padStart(5)}x  ${k.slice(0, 150)}`,
      );
  };
  return out instanceof Promise
    ? out.then((v) => {
        counting = false;
        finish(performance.now() - t);
        return v;
      })
    : (finish(performance.now() - t), out);
};
const { newContext } = await load("src/context.js");
const ctx = () => newContext(core, key);
const mailOf = (id) =>
  core.openMessagesFor ? core.openMessagesFor(id) : core.messagesFor(id);
const pms = core
  .listAgents()
  .filter((a) => a.kind === "PM" && a.state === "active");
console.log(`agents ${core.listAgents().length}, active PM ${pms.length}`);
// background loop bodies, in the order the daemon runs them
const relay = () => {
  if (core.activeAgents) core.activeAgents();
  else core.listAgents();
  core.unannouncedReports(key);
  core.unannouncedReviews(key);
  core.unannouncedPlanNotices(key);
  core.unannouncedFindingNotices(key);
};
report(
  "report relay tick, first pass (what a restarted daemon pays once)",
  relay,
);
report("report relay tick, steady state (every 2 s)", relay);
const { loadCapstanConfig } = await load("src/config/capstan-config.js");
const { findingCheckSeconds: _unused, ...timers } =
  loadCapstanConfig(root).timers;
// What one driver tick reads from the ledger: the active agents, advanceMessaging, queueMissingDeliveryNotices and the
// PM's mail (the old driver read all of it in #judgeWakes and #judgeStaleFor; the new one reads the open part once,
// and #judgeWakes reads nothing while no wake is outstanding).
const driverTick = () => {
  if (core.activeAgents) core.activeAgents();
  else core.listAgents();
  try {
    core.advanceMessaging(ctx(), timers);
  } catch (e) {
    console.log("advance:", String(e).slice(0, 80));
  }
  core.queueMissingDeliveryNotices(ctx());
  for (let i = 0; i < (core.openMessagesFor ? 1 : 2); i++)
    for (const pm of pms) mailOf(pm.agentId);
};
report(
  "driver tick, first pass (also applies the transitions that are due)",
  driverTick,
);
report("driver tick, steady state (every 2 s)", driverTick);
// the status command through the real socket
const socketPath = path.join(state, "control.sock");
const { createCommandHandlers } = await load("src/commands.js");
const commands = createCommandHandlers({
  core,
  controllerCredential: key,
  controllerLocation: {
    projectRoot: root,
    ledgerPath: path.join(state, "controller.sqlite"),
  },
  driverSnapshot: () => ({ stalledAgentIds: [], stuck: [] }),
  log: () => {},
  inspectCommit: async () => {
    throw new Error("n/a");
  },
  newCommitMessages: async () => [],
  commitExists: async () => false,
  integrationGit: {},
});
const server = await startDaemonServer({
  socketPath,
  core,
  log: () => {},
  onShutdown: () => {},
  location: {
    projectRoot: root,
    ledgerPath: path.join(state, "controller.sqlite"),
  },
  commands,
});
for (let i = 0; i < 3; i++) await callDaemon(socketPath, key, "status");
await report("status command (over the socket)", () =>
  callDaemon(socketPath, key, "status"),
);
const lat = [];
for (let i = 0; i < 30; i++) {
  const t = performance.now();
  await callDaemon(socketPath, key, "status");
  lat.push(performance.now() - t);
}
lat.sort((a, b) => a - b);
console.log(
  `\nstatus latency in-process n=30 p50=${lat[15].toFixed(1)}ms p95=${lat[28].toFixed(1)}ms`,
);
await server.drain?.();
server.cleanup?.();
core.close();
process.exit(0);

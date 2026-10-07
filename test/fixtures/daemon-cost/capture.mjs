// Captures what the operator sees from a ledger: status, inbox and inspect results through the real daemon
// command handlers, plus the outputs of the background loops (plan notices, messaging advance, delivery
// notices, PM mail), with a fixed clock. It runs against ANY build (`distDir`), which is how the goldens were
// made with the code before the daemon-profile fixes and how the current code is compared with them.
//   node export.mjs <dist dir> <out.json>        writes the goldens for the generated ledger
// Never run against a live ledger directly: give it a copy (scripts/copy-ledger.mjs).
import path from "node:path";
import { pathToFileURL } from "node:url";

export const FIXED_NOW = Date.parse("2026-06-01T00:00:00.000Z");

/** Counts the rows and statements of the Statement calls made while `fn` runs (via the build's own adapter). */
export async function withCounters(distDir, fn) {
  const sqlite = await import(
    pathToFileURL(path.join(distDir, "src/controller/sqlite.js")).href
  );
  const proto = sqlite.Statement.prototype;
  const counters = { statements: 0, rows: 0 };
  const originals = {};
  for (const method of ["get", "all", "run"]) {
    originals[method] = proto[method];
    proto[method] = function (...args) {
      const result = originals[method].apply(this, args);
      counters.statements += 1;
      if (method === "all") counters.rows += result.length;
      else if (method === "get" && result !== undefined) counters.rows += 1;
      return result;
    };
  }
  try {
    return { value: await fn(counters), counters };
  } finally {
    for (const method of Object.keys(originals))
      proto[method] = originals[method];
  }
}

export async function openFor(
  distDir,
  { stateDirectory, projectId, name, credential, workspaceRoot },
) {
  const load = (p) => import(pathToFileURL(path.join(distDir, p)).href);
  const { ControllerCore } = await load("src/controller/core.js");
  const core = await ControllerCore.open({
    stateDirectory,
    workspaceRoot,
    clock: () => new Date(FIXED_NOW),
    project: {
      projectId,
      name,
      ownerCredential: credential,
      initialInputs: [],
    },
  });
  return core;
}

/** The command handlers behind a socket, as the daemon builds them (no driver, no launcher, a fixed clock). */
export async function serve(
  distDir,
  core,
  { stateDirectory, workspaceRoot, credential },
) {
  const load = (p) => import(pathToFileURL(path.join(distDir, p)).href);
  const { createCommandHandlers } = await load("src/commands.js");
  const { startDaemonServer } = await load("src/daemon.js");
  const { callDaemon } = await load("src/client.js");
  const location = {
    projectRoot: workspaceRoot,
    ledgerPath: path.join(stateDirectory, "controller.sqlite"),
  };
  const commands = createCommandHandlers({
    core,
    controllerCredential: credential,
    controllerLocation: location,
    now: () => FIXED_NOW,
    driverSnapshot: () => ({ stalledAgentIds: [], stuck: [] }),
    log: () => {},
    inspectCommit: async () => {
      throw new Error("not available");
    },
    newCommitMessages: async () => [],
    commitExists: async () => false,
    integrationGit: {},
  });
  const socketPath = path.join(stateDirectory, "control.sock");
  const server = await startDaemonServer({
    socketPath,
    core,
    log: () => {},
    onShutdown: () => {},
    location,
    commands,
  });
  const call = async (command, args = []) => {
    const { response } = await callDaemon(
      socketPath,
      credential,
      command,
      args,
    );
    return response;
  };
  return {
    call,
    close: async () => {
      server.stopAccepting?.();
      await server.drain?.();
      server.cleanup?.();
    },
  };
}

/** One tick of the report relay's reads, as src/reports.ts runs them (an older build lists every agent). */
export function relayPass(core, credential) {
  const agents = core.activeAgents ? core.activeAgents() : core.listAgents();
  core.unannouncedReports(credential);
  core.unannouncedReviews(credential);
  core.unannouncedPlanNotices(credential);
  core.unannouncedFindingNotices(credential);
  return agents.length;
}

/**
 * The ledger work of one driver tick that does not need Herdr: advance, delivery notices and the PM's mail. An older
 * build reads all of the PM's messages twice (`#judgeWakes`, `#judgeStaleFor`); this one reads the open ones once.
 */
export async function driverPass(distDir, core, credential, timers, pmAgentId) {
  const { newContext } = await import(
    pathToFileURL(path.join(distDir, "src/context.js")).href
  );
  core.advanceMessaging(newContext(core, credential), timers);
  core.queueMissingDeliveryNotices(newContext(core, credential));
  if (core.openMessagesFor) core.openMessagesFor(pmAgentId);
  else for (let i = 0; i < 2; i++) core.messagesFor(pmAgentId);
}

/** Everything compared between builds. `inspectIds` are ids of rows that exist in the ledger. */
export async function capture(
  distDir,
  core,
  served,
  { credential, timers, inspectIds, mailAgentIds },
) {
  const load = (p) => import(pathToFileURL(path.join(distDir, p)).href);
  const { newContext } = await load("src/context.js");
  const { pmMailSummary } = await load("src/pm-mail.js");
  const out = {};
  out.status = await served.call("status");
  out.inbox = {};
  for (const id of mailAgentIds)
    out.inbox[id] = await served.call("inbox", [id]);
  out.inspect = {};
  for (const id of inspectIds) out.inspect[id] = core.inspect(id);
  const nonFinal = (rows) =>
    rows.filter((m) => !["acked", "acked_late", "cancelled"].includes(m.state));
  const openMail = (id) =>
    core.openMessagesFor
      ? core.openMessagesFor(id)
      : nonFinal(core.messagesFor(id));
  out.loops = {
    planNotices: core.unannouncedPlanNotices(credential),
    reportNotices: core.unannouncedReports(credential),
    reviewNotices: core.unannouncedReviews(credential),
    pmMail: Object.fromEntries(
      mailAgentIds.map((id) => [
        id,
        {
          open: openMail(id),
          summary: pmMailSummary(openMail(id), FIXED_NOW, 600),
        },
      ]),
    ),
    unresolved: core.unresolvedMessages(credential, 50),
    advance: core.advanceMessaging(newContext(core, credential), timers),
    deliveryNotices: core.queueMissingDeliveryNotices(
      newContext(core, credential),
    ),
  };
  return out;
}

/** Makes the parts that name this run (paths, pid, wall clock) comparable. */
export function normalise(value, replacements = {}) {
  const text = JSON.stringify(value, (key, v) => {
    if (key === "pid" && typeof v === "number") return "<pid>";
    if (typeof v === "string") {
      let s = v;
      for (const [from, to] of Object.entries(replacements))
        s = s.split(from).join(to);
      return s;
    }
    return v;
  });
  return JSON.parse(text);
}

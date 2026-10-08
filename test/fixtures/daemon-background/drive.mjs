// Runs a real DeliveryDriver for a fixed number of ticks on the generated large ledger (test/fixtures/daemon-cost) with a
// scripted Herdr stand-in and a virtual clock, and returns everything observable: the calls made to Herdr and to the process
// probe (the spawns of a live daemon), the driver's own log, its snapshot, the notifications, the ledger rows it changed, and
// the ledger statements each tick used. It runs against ANY build (`distDir`), which is how the golden was made with the code
// before the daemon-background fixes (git 8b04ed0) and how the current code is compared with it.
//   node drive.mjs <dist dir> <out.json>     writes the golden
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { buildLedger } from "../daemon-cost/ledger.mjs";

export const START = Date.parse("2026-06-01T00:00:00.000Z");
export const TICK_MS = 2000;
export const TICKS = 80;
const WORKING = new Set(["developer-494", "developer-495", "developer-499"]);

/** Makes the parts that name this run comparable: the directory, and random ids by first appearance. */
function stable(value, dir) {
  const ids = new Map();
  const text = JSON.stringify(value)
    .split(dir)
    .join("<dir>")
    .replace(
      /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g,
      (id) => {
        if (!ids.has(id)) ids.set(id, `<id-${ids.size + 1}>`);
        return ids.get(id);
      },
    );
  return JSON.parse(text);
}

export async function drive(distDir, dir) {
  const load = (p) => import(pathToFileURL(path.join(distDir, p)).href);
  const built = await buildLedger(distDir, dir);
  const { ControllerCore } = await load("src/controller/core.js");
  const { DeliveryDriver } = await load("src/driver.js");
  const { newContext } = await load("src/context.js");
  const { openSqlite } = await load("src/controller/sqlite.js");
  const sqlite = await load("src/controller/sqlite.js");
  let now = START;
  const core = await ControllerCore.open({
    stateDirectory: built.stateDirectory,
    workspaceRoot: built.workspaceRoot,
    clock: () => new Date(now),
    project: {
      projectId: built.projectId,
      name: built.name,
      ownerCredential: built.credential,
      initialInputs: [],
    },
  });
  const agents = core.activeAgents().map((a) => a.agentId);
  const pane = (id) => `w1:${id}`;
  const calls = [];
  let tick = 0;
  const record = (kind, detail) => calls.push([tick, kind, detail]);
  const adapter = {
    paneForAgent: (id) => (agents.includes(id) ? pane(id) : undefined),
    paneEntry: (paneId) => ({ agent: paneId.slice(3) }),
    async agentObservation(id) {
      record("agent_get", id);
      return WORKING.has(id) ? "working" : "idle";
    },
    async guardedSend({ paneId, text, beforeSend }) {
      record("send", [paneId, text.split("\n")[0]]);
      if (WORKING.has(paneId.slice(3)))
        return { sent: false, reason: "agent_busy" };
      await beforeSend();
      return { sent: true };
    },
    async wakePm({ paneId, text, beforeSend }) {
      record("wake", [paneId, text]);
      await beforeSend();
      return { sent: true };
    },
    async clearAfterDeferral() {
      throw new Error("not scripted");
    },
  };
  let cpu = 0;
  const processProbe = {
    async sample(paneId) {
      record("process_info", paneId);
      cpu += paneId.endsWith("494") ? 500 : 0;
      return {
        processes: [
          { pid: 100, ppid: 1, comm: "sh", cpuMs: cpu, startKey: "1" },
        ],
      };
    },
  };
  const notifications = [];
  const notifier = {
    async send(request) {
      notifications.push(["send", tick, request.kind, request.messageId]);
      return [{ channel: "herdr", ok: true }];
    },
    write(request, results, recorded) {
      notifications.push([
        "write",
        tick,
        request.kind,
        request.messageId,
        recorded,
      ]);
    },
  };
  const log = [];
  const driver = new DeliveryDriver({
    core,
    adapter,
    timers: built.timers,
    notifier,
    credential: built.credential,
    now: () => now,
    log: (event, details) => log.push([tick, event, details]),
    processProbe,
    pmStaleSeconds: 60,
  });
  const ctx = () => newContext(core, built.credential);
  const statements = [];
  const rowCounts = [];
  const proto = sqlite.Statement.prototype;
  const originals = {};
  let counted = 0;
  let countedRows = 0;
  for (const method of ["get", "all", "run"]) {
    originals[method] = proto[method];
    proto[method] = function (...args) {
      counted += 1;
      const out = originals[method].apply(this, args);
      if (method === "all") countedRows += out.length;
      else if (method === "get" && out !== undefined) countedRows += 1;
      return out;
    };
  }
  try {
    for (tick = 0; tick < TICKS; tick++) {
      now = START + tick * TICK_MS;
      // The scripted day: the ledger's own unread PM mail (message-1046) is pulled at tick 30, two more mails to the PM arrive, one to a busy developer, one to an idle one.
      if (tick === 2)
        core.enqueueMessage(ctx(), {
          recipientAgentId: "pm-1",
          body: "first note for the PM",
        });
      if (tick === 2)
        core.enqueueMessage(ctx(), {
          recipientAgentId: "developer-494",
          body: "note for a busy developer",
        });
      if (tick === 3)
        core.enqueueMessage(ctx(), {
          recipientAgentId: "developer-497",
          body: "note for an idle developer",
        });
      if (tick === 30) core.recordSent(ctx(), "message-1046");
      if (tick === 35)
        core.enqueueMessage(ctx(), {
          recipientAgentId: "pm-1",
          body: "second note for the PM",
        });
      counted = 0;
      countedRows = 0;
      await driver.tick();
      statements.push(counted);
      rowCounts.push(countedRows);
    }
  } finally {
    for (const method of Object.keys(originals))
      proto[method] = originals[method];
  }
  const rows = core
    .openMessagesFor("pm-1")
    .concat(
      core.openMessagesFor("developer-494"),
      core.openMessagesFor("developer-497"),
    )
    .map((m) => ({
      body: m.body,
      state: m.state,
      deferredReason: m.deferredReason,
      deferralCount: m.deferralCount,
    }));
  const snapshot = driver.snapshot();
  core.close();
  const db = openSqlite(path.join(built.stateDirectory, "controller.sqlite"));
  const events = db
    .prepare(
      "SELECT entity_type, to_state, count(*) AS c FROM controller_events WHERE created_at >= ? GROUP BY entity_type, to_state ORDER BY entity_type, to_state",
    )
    .all(new Date(START).toISOString());
  db.close();
  return {
    result: stable({ calls, log, notifications, snapshot, rows, events }, dir),
    statements,
    rowCounts,
  };
}

if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === new URL(import.meta.url).pathname
) {
  const [distDir, out] = process.argv.slice(2);
  if (!distDir || !out)
    throw new Error("usage: drive.mjs <dist dir> <out.json>");
  const dir = fs.mkdtempSync(
    path.join(os.tmpdir(), "capstan-daemon-background-"),
  );
  try {
    const { result, statements, rowCounts } = await drive(distDir, dir);
    fs.writeFileSync(out, `${JSON.stringify(result, null, 1)}\n`);
    console.log(`wrote ${out}`);
    console.log(`statements per tick: ${JSON.stringify(statements)}`);
    console.log(`rows per tick: ${JSON.stringify(rowCounts)}`);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  process.exit(0);
}

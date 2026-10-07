// Builds the generated large ledger with a given build (`distDir`), so the same ledger can be made by the code
// before and after the daemon-profile fixes: the project is created through that build's ControllerCore (its
// schema), then filled with generate.mjs through that build's adapter.
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { seedLargeLedger } from "./generate.mjs";

const KINDS = [
  "project_config",
  "task_brief",
  "acceptance_criteria",
  "policy",
  "plan",
];

export async function buildLedger(distDir, dir) {
  const load = (p) => import(pathToFileURL(path.join(distDir, p)).href);
  const { ControllerCore } = await load("src/controller/core.js");
  const { openSqlite } = await load("src/controller/sqlite.js");
  const stateDirectory = path.join(dir, "state");
  fs.mkdirSync(stateDirectory, { recursive: true, mode: 0o700 });
  const project = {
    projectId: "pdaemoncostledger",
    name: "Daemon cost ledger",
    ownerCredential: "owner-daemon-cost-ledger-0123456789abcdef",
    initialInputs: KINDS.map((kind) => ({
      kind,
      content:
        kind === "acceptance_criteria"
          ? ["criterion-one"]
          : { kind, revision: 1 },
    })),
  };
  (await ControllerCore.open({ stateDirectory, project })).close();
  const db = openSqlite(path.join(stateDirectory, "controller.sqlite"));
  try {
    seedLargeLedger(db, project.projectId);
  } finally {
    db.close();
  }
  return {
    stateDirectory,
    workspaceRoot: dir,
    projectId: project.projectId,
    name: project.name,
    credential: project.ownerCredential,
    timers: {
      maxDeferralSeconds: 120,
      maxBusyDeferralSeconds: 3600,
      pmAckTimeoutSeconds: 600,
      pmNotifyAfterSeconds: 300,
      notifyIntervalSeconds: 600,
      stallAfterSeconds: 900,
      workerAckTimeoutSeconds: 600,
      pmWakeAfterSeconds: 20,
      pmWakeIntervalSeconds: 120,
    },
    inspectIds: ["report-5", "report-199", "review-5", "int-2"],
    mailAgentIds: ["pm-1", "developer-499", "developer-498"],
  };
}

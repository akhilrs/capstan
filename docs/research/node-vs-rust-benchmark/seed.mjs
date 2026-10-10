// Registers a PM (pm-1) and N developers (developer-1..N) in the ledger of a scratch project that no daemon owns, with the
// v0.3.0 Node core (the unpacked capstan-controller-0.3.0.tgz plus `npm ci --omit=dev`), and prints the tokens as JSON:
// {"pm-1": "<token>", "developer-1": "<token>", ...}. Both sides are seeded with this same code, so both start from the same ledger.
// Usage: node seed.mjs <unpacked package dir> <project root> [developers=10]. Scratch projects under /tmp/capstan-cutover* only.
import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [repository, root, count = "10"] = process.argv.slice(2);
if (!repository || !root) {
  console.error("usage: node seed.mjs <package dir> <project root> [developers]");
  process.exit(2);
}
if (!root.startsWith("/tmp/capstan-cutover")) {
  console.error("seed.mjs only opens scratch projects under /tmp/capstan-cutover*");
  process.exit(2);
}
const { ControllerCore } = await import(
  pathToFileURL(path.join(repository, "dist/src/controller/core.js")).href
);
const config = JSON.parse(readFileSync(path.join(root, ".capstan", "project.json"), "utf8"));
const owner = readFileSync(path.join(root, ".capstan", "operator.key"), "utf8").trim();
const kinds = ["project_config", "task_brief", "acceptance_criteria", "policy", "plan"];
const core = await ControllerCore.open({
  stateDirectory: config.stateDirectory,
  project: {
    projectId: config.projectId,
    name: config.name,
    ownerCredential: owner,
    initialInputs: kinds.map((kind) => ({
      kind,
      content: kind === "acceptance_criteria" ? ["criterion"] : { kind, revision: 1 },
    })),
  },
});
let n = 0;
const mutation = () => ({
  credential: owner,
  requestId: `req-seed-${(n += 1)}`,
  idempotencyKey: `idem-seed-${n}`,
  expectedVersion: core.stateVersion,
  inputRevision: core.inputRevision,
});
try {
  core.syncRoleDefinitions(mutation(), [
    { name: "pm", kind: "PM", host: "claude", configHash: "a".repeat(64) },
    { name: "developer", kind: "Developer", host: "claude", configHash: "b".repeat(64) },
  ]);
  const out = {};
  const add = (agentId, roleName, kind) => {
    const seatId = `seat-${agentId}`;
    core.createSeat(mutation(), { seatId, name: seatId, role: kind });
    const actor = core.createActor(mutation(), { displayName: agentId, role: kind, seatId });
    core.registerAgent(mutation(), { agentId, roleName, seatId, actorId: actor.actorId });
    out[agentId] = actor.credential;
  };
  add("pm-1", "pm", "PM");
  for (let i = 1; i <= Number(count); i += 1) add(`developer-${i}`, "developer", "Developer");
  console.log(JSON.stringify(out));
} finally {
  core.close();
}

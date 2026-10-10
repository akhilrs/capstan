// Registers a PM and a developer in the ledger of a scratch project that no daemon owns, with the frozen Node build, and
// prints their tokens as JSON: {"pm": {...}, "developer": {...}}. Usage: node seed.mjs <repository> <project root>
// Same registration as test/cli-parity-harness.ts. Scratch projects only: it opens the ledger of the project it is given.
import { readFileSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [repository, root] = process.argv.slice(2);
if (!repository || !root) {
  console.error("usage: node seed.mjs <repository> <project root>");
  process.exit(2);
}
if (!root.startsWith("/tmp/capstan-cutover")) {
  console.error("seed.mjs only opens scratch projects under /tmp/capstan-cutover*");
  process.exit(2);
}
const { ControllerCore } = await import(
  pathToFileURL(path.join(repository, "dist/src/controller/core.js")).href
);
const config = JSON.parse(
  readFileSync(path.join(root, ".capstan", "project.json"), "utf8"),
);
const owner = readFileSync(path.join(root, ".capstan", "operator.key"), "utf8").trim();
const kinds = ["project_config", "task_brief", "acceptance_criteria", "policy", "plan"];
const project = {
  projectId: config.projectId,
  name: config.name,
  ownerCredential: owner,
  initialInputs: kinds.map((kind) => ({
    kind,
    content: kind === "acceptance_criteria" ? ["criterion"] : { kind, revision: 1 },
  })),
};
const core = await ControllerCore.open({ stateDirectory: config.stateDirectory, project });
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
  for (const [name, kind] of [["pm", "PM"], ["developer", "Developer"]]) {
    const seatId = `${name}-seat`;
    core.createSeat(mutation(), { seatId, name, role: kind });
    const actor = core.createActor(mutation(), { displayName: name, role: kind, seatId });
    core.registerAgent(mutation(), { agentId: `${name}-1`, roleName: name, seatId, actorId: actor.actorId });
    out[name] = { agentId: `${name}-1`, token: actor.credential };
  }
  console.log(JSON.stringify(out));
} finally {
  core.close();
}
